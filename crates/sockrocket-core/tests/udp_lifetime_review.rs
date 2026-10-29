#[cfg(test)]
mod udp_lifetime {
    use sockrocket_core::proxy::{
        ProxyStats,
        connector::{BoxProxyStream, Outbound, SharedOutbound},
        datagram::{BoxDatagram, Datagram, DatagramFuture},
        socks5::Socks5Server,
    };
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpStream, UdpSocket},
        sync::watch,
        time::{Duration, timeout},
    };
    struct Counts {
        created: AtomicUsize,
        ended: AtomicUsize,
        dropped: AtomicUsize,
    }
    struct Mock(Arc<Counts>);
    struct DeadDatagram(Arc<Counts>);
    impl Drop for DeadDatagram {
        fn drop(&mut self) {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Datagram for DeadDatagram {
        fn send<'a>(&'a self, _: &'a [u8]) -> DatagramFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn recv<'a>(&'a self, _: &'a mut [u8]) -> DatagramFuture<'a, usize> {
            Box::pin(async move {
                self.0.ended.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("synthetic receive failure")
            })
        }
    }
    impl Outbound for Mock {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>> {
            Box::pin(async { anyhow::bail!("unused") })
        }
        fn connect_datagram(&self, _: &str, _: u16) -> DatagramFuture<'_, BoxDatagram> {
            Box::pin(async move {
                self.0.created.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(DeadDatagram(self.0.clone())) as BoxDatagram)
            })
        }
        fn name(&self) -> &str {
            "memory-review"
        }
    }
    #[tokio::test]
    async fn completed_udp_tasks_are_reclaimed_without_another_client_packet() {
        let counts = Arc::new(Counts {
            created: AtomicUsize::new(0),
            ended: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        });
        let server = Socks5Server::bind(
            "127.0.0.1",
            0,
            SharedOutbound(Arc::new(Mock(counts.clone()))),
            ProxyStats::default(),
        )
        .await
        .unwrap();
        let port = server.local_port();
        let (stop, rx) = watch::channel(false);
        let task = tokio::spawn(server.run(rx));
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut controls = Vec::new();
        for n in 1..=32 {
            let mut control = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            control.write_all(&[5, 1, 0]).await.unwrap();
            let mut greeting = [0; 2];
            control.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 0]);
            control
                .write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            let mut reply = [0; 10];
            control.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply[..4], &[5, 0, 0, 1]);
            let udp_port = u16::from_be_bytes([reply[8], reply[9]]);
            socket
                .send_to(
                    &[0, 0, 0, 1, 192, 0, 2, 1, 0, 53, 1],
                    ("127.0.0.1", udp_port),
                )
                .await
                .unwrap();
            timeout(Duration::from_secs(2), async {
                while counts.ended.load(Ordering::SeqCst) < n {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            tokio::task::yield_now().await;
            controls.push(control);
        }
        assert_eq!(counts.created.load(Ordering::SeqCst), 32);
        timeout(Duration::from_secs(3), async {
            while counts.dropped.load(Ordering::SeqCst) < 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finished remote objects were retained without further UDP traffic");

        stop.send(true).unwrap();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(counts.dropped.load(Ordering::SeqCst), 32);
    }
}
