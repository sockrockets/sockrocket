//! Compression related operations

use crate::msgs::enums::CertificateCompressionAlgorithm;
use alloc::vec::Vec;
use brotli::DecompressorWriter;
use brotli::{enc::BrotliEncoderParams, CompressorWriter};
use core::fmt::Debug;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{Read, Result, Write};

/// A certificate compression algorithm described
/// as a pair of compression and decompression
/// functions used to compress and decompress a certificate\
#[derive(Debug)]
pub struct CertificateCompression {
    /// compression algorithm
    pub alg: CertificateCompressionAlgorithm,

    /// compression provider
    pub provider: &'static dyn CompressionProvider,
}

/// todo
pub trait CompressionProvider: Send + Sync + Debug {
    /// todo
    fn compress(&self, _writer: Vec<u8>, _input: &[u8]) -> Result<Vec<u8>> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// todo
    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>>;
}

/// todo
#[derive(Debug)]
pub struct BrotliParams {
    /// todo
    pub buffer_size: usize,
    /// todo
    pub params: BrotliEncoderParams,
}

impl CompressionProvider for BrotliParams {
    fn compress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        let mut compressor = CompressorWriter::with_params(writer, self.buffer_size, &self.params);
        compressor.write_all(input)?;
        compressor.flush()?;
        Ok(compressor.into_inner())
    }

    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_brotli(writer, input, self.buffer_size)
    }
}

/// todo
mod compression_params {
    use brotli::enc::BrotliEncoderParams;

    #[allow(non_snake_case)]
    const fn BROTLI_DISTANCE_ALPHABET_SIZE(NPOSTFIX: u32, NDIRECT: u32, MAXNBITS: u32) -> u32 {
        brotli::enc::encode::BROTLI_NUM_DISTANCE_SHORT_CODES
            + (NDIRECT)
            + ((MAXNBITS) << ((NPOSTFIX) + 1))
    }

    pub(crate) const BROTLI_ENCODER_DEFAULT: BrotliEncoderParams = BrotliEncoderParams {
        dist: brotli::enc::command::BrotliDistanceParams {
            distance_postfix_bits: 0,
            num_direct_distance_codes: 0,
            alphabet_size: BROTLI_DISTANCE_ALPHABET_SIZE(
                0,
                0,
                brotli::enc::encode::BROTLI_MAX_DISTANCE_BITS,
            ),
            max_distance: brotli::enc::encode::BROTLI_MAX_DISTANCE,
        },
        mode: brotli::enc::backward_references::BrotliEncoderMode::BROTLI_MODE_GENERIC,
        log_meta_block: false,
        large_window: false,
        avoid_distance_prefix_search: false,
        quality: 11,
        q9_5: false,
        lgwin: 22i32,
        lgblock: 0i32,
        size_hint: 0usize,
        disable_literal_context_modeling: 0i32,
        stride_detection_quality: 0,
        high_entropy_detection_quality: 0,
        cdf_adaptation_detection: 0,
        prior_bitmask_detection: 0,
        literal_adaptation: [(0, 0); 4],
        catable: false,
        use_dictionary: true,
        appendable: false,
        magic_number: false,
        favor_cpu_efficiency: false,
        hasher: brotli::enc::backward_references::BrotliHasherParams {
            type_: 6,
            block_bits: 9 - 1,
            bucket_bits: 15,
            hash_len: 5,
            num_last_distances_to_check: 16,
            literal_byte_score: 0,
        },
    };

    pub(crate) const ZLIB_ENCODER_DEFAULT: flate2::Compression = flate2::Compression::new(6);

    #[test]
    fn param_eq() {
        assert_eq!(
            format!("{BROTLI_ENCODER_DEFAULT:?}"),
            format!("{:?}", BrotliEncoderParams::default())
        );

        assert_eq!(
            format!("{ZLIB_ENCODER_DEFAULT:?}"),
            format!("{:?}", flate2::Compression::default())
        );
    }
}

/// todo
pub static BROTLI_DEFAULT: &CertificateCompression = &CertificateCompression {
    alg: CertificateCompressionAlgorithm::Brotli,
    provider: &BrotliParams {
        buffer_size: 4096,
        params: compression_params::BROTLI_ENCODER_DEFAULT,
    },
};

#[derive(Debug)]
/// todo
pub struct ZlibParams {
    /// Must between 0-9 (inclusive)
    pub compression_level: flate2::Compression,
}

impl CompressionProvider for ZlibParams {
    fn compress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        let mut compressor = ZlibEncoder::new(writer, self.compression_level);
        compressor.write_all(input)?;
        compressor.flush()?;
        compressor.finish()
    }

    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_zlib(writer, input)
    }
}

/// todo
pub static ZLIB_DEFAULT: &CertificateCompression = &CertificateCompression {
    alg: CertificateCompressionAlgorithm::Zlib,
    provider: &ZlibParams {
        compression_level: compression_params::ZLIB_ENCODER_DEFAULT,
    },
};

#[derive(Debug)]
/// todo
pub struct ZstdParams {
    /// todo
    pub compression_level: u32,
}

impl CompressionProvider for ZstdParams {
    fn compress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        let mut compressor = zstd::Encoder::new(writer, self.compression_level as i32)?;
        compressor.write_all(input)?;
        compressor.flush()?;
        compressor.finish()
    }

    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_zstd(writer, input)
    }
}

/// todo
pub static ZSTD_DEFAULT: &CertificateCompression = &CertificateCompression {
    alg: CertificateCompressionAlgorithm::Zstd,
    provider: &ZstdParams {
        compression_level: zstd::DEFAULT_COMPRESSION_LEVEL as u32,
    },
};

fn decompress_brotli(writer: Vec<u8>, input: &[u8], buffer_size: usize) -> Result<Vec<u8>> {
    let mut decompressor = DecompressorWriter::new(writer, buffer_size);
    decompressor.write_all(input)?;
    decompressor.flush()?;
    decompressor.into_inner().map_err(|_| std::io::ErrorKind::InvalidData.into())
}
fn decompress_zlib(mut writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
    ZlibDecoder::new(input).read_to_end(&mut writer)?;
    Ok(writer)
}
fn decompress_zstd(mut writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
    zstd::Decoder::new(input)?.read_to_end(&mut writer)?;
    Ok(writer)
}

// ClientHello advertises the ability to decode the server's certificate.
// Separate vtables avoid retaining unused encoder code in client binaries.
// Both provider variants call the very same decompression functions.
#[derive(Debug)]
struct BrotliClient;
impl CompressionProvider for BrotliClient {
    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_brotli(writer, input, 4096)
    }
}
#[derive(Debug)]
struct ZlibClient;
impl CompressionProvider for ZlibClient {
    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_zlib(writer, input)
    }
}
#[derive(Debug)]
struct ZstdClient;
impl CompressionProvider for ZstdClient {
    fn decompress(&self, writer: Vec<u8>, input: &[u8]) -> Result<Vec<u8>> {
        decompress_zstd(writer, input)
    }
}

/// A receive-only certificate codec for a TLS client fingerprint. Encoding
/// returns Unsupported; the full *_DEFAULT providers remain available for
/// callers that need both operations. The advertised algorithms are unchanged.
pub fn client_certificate_decompressor(alg: CertificateCompressionAlgorithm) -> Option<&'static CertificateCompression> {
    static BROTLI: CertificateCompression = CertificateCompression { alg: CertificateCompressionAlgorithm::Brotli, provider: &BrotliClient };
    static ZLIB: CertificateCompression = CertificateCompression { alg: CertificateCompressionAlgorithm::Zlib, provider: &ZlibClient };
    static ZSTD: CertificateCompression = CertificateCompression { alg: CertificateCompressionAlgorithm::Zstd, provider: &ZstdClient };
    match alg {
        CertificateCompressionAlgorithm::Brotli => Some(&BROTLI),
        CertificateCompressionAlgorithm::Zlib => Some(&ZLIB),
        CertificateCompressionAlgorithm::Zstd => Some(&ZSTD),
        _ => None,
    }
}
