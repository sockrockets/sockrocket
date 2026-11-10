//! V2Ray gRPC Hunk framing. Both Tun and TunMulti use field 1 for data.
use super::connector::BoxProxyStream;
use crate::config::model::GrpcConfig;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::{
    codec::{Decoder, Encoder, Framed},
    io::{SinkWriter, StreamReader},
};

const MAX_MESSAGE: usize = 4 * 1024 * 1024;

pub(super) async fn connect(
    stream: BoxProxyStream,
    host: &str,
    secure: bool,
    config: &GrpcConfig,
) -> anyhow::Result<BoxProxyStream> {
    let method = if config.multi_mode { "TunMulti" } else { "Tun" };
    let path = format!("/{}/{method}", urlencoding::encode(&config.service_name));
    let uri = http::Uri::builder()
        .scheme(if secure { "https" } else { "http" })
        .authority(config.authority.as_deref().unwrap_or(host))
        .path_and_query(path)
        .build()?;
    let request = http::Request::post(uri)
        .version(http::Version::HTTP_2)
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(())?;
    let raw = super::h2_stream::open(stream, request).await?;
    Ok(Box::new(GrpcStream {
        io: SinkWriter::new(StreamReader::new(Framed::new(raw, Hunks::default()))),
    }))
}

// An application may read its greeting immediately after writing the protocol header.
// Flush the framed sink before a read so small requests do not wait for another write.
struct GrpcStream {
    io: SinkWriter<StreamReader<Framed<BoxProxyStream, Hunks>, Bytes>>,
}
impl AsyncRead for GrpcStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        // Keep reads live while the HTTP/2 send window is exhausted. The flush
        // registers this waker, and polling the receive side releases window credit.
        if let Poll::Ready(Err(e)) = Pin::new(&mut self.io).poll_flush(cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for GrpcStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        ready!(Pin::new(&mut self.io).poll_flush(cx))?;
        let n = ready!(Pin::new(&mut self.io).poll_write(cx, &data[..data.len().min(16384)]))?;
        if let Poll::Ready(Err(e)) = Pin::new(&mut self.io).poll_flush(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[derive(Default)]
struct Hunks {
    message: Bytes,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid gRPC Hunk")
}
impl Decoder for Hunks {
    type Error = io::Error;
    type Item = Bytes;
    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Bytes>> {
        loop {
            if self.message.is_empty() {
                if src.len() < 5 {
                    return Ok(None);
                }
                let len = u32::from_be_bytes(src[1..5].try_into().unwrap()) as usize;
                if src[0] != 0 || len > MAX_MESSAGE {
                    return Err(invalid());
                }
                if src.len() - 5 < len {
                    return Ok(None);
                }
                src.advance(5);
                self.message = src.split_to(len).freeze();
                if len == 0 {
                    continue;
                }
            }
            if self.message.get_u8() != 10 {
                return Err(invalid());
            }
            let mut length = 0usize;
            for shift in (0..28).step_by(7) {
                if self.message.is_empty() {
                    return Err(invalid());
                }
                let byte = self.message.get_u8();
                length |= usize::from(byte & 127) << shift;
                if byte < 128 {
                    if length > self.message.len() {
                        return Err(invalid());
                    }
                    if length > 0 {
                        return Ok(Some(self.message.split_to(length)));
                    }
                    break;
                }
                if shift == 21 {
                    return Err(invalid());
                }
            }
        }
    }
}

impl Encoder<&[u8]> for Hunks {
    type Error = io::Error;
    fn encode(&mut self, data: &[u8], dst: &mut BytesMut) -> io::Result<()> {
        for payload in data.chunks(16 * 1024) {
            let len = payload.len();
            let varint_len = if len < 128 {
                1
            } else if len < 16384 {
                2
            } else {
                3
            };
            dst.reserve(6 + varint_len + len);
            dst.put_u8(0);
            dst.put_u32((1 + varint_len + len) as u32);
            dst.put_u8(10);
            let mut n = len;
            while n >= 128 {
                dst.put_u8(n as u8 | 128);
                n >>= 7;
            }
            dst.put_u8(n as u8);
            dst.extend_from_slice(payload);
        }
        Ok(())
    }
}
