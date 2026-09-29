use rust_raknet::{NetherNetProxy, RaknetListener, RaknetSocket, Reliability};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};

#[tokio::test]
async fn foreign_udp_datagram_cannot_disconnect_a_client() {
    timeout(Duration::from_secs(5), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        listener.listen().await;
        let client = RaknetSocket::connect(&addr).await.unwrap();
        let server = listener.accept().await.unwrap();
        let foreign = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target =
            std::net::SocketAddr::from(([127, 0, 0, 1], client.local_addr().unwrap().port()));
        foreign.send_to(&[0x15], target).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        server
            .send(&[0xfe, 42], Reliability::ReliableOrdered)
            .await
            .unwrap();
        assert_eq!(client.recv().await.unwrap(), vec![0xfe, 42]);
        client.close().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
    })
    .await
    .expect("foreign datagram interrupted the real connection");
}

#[tokio::test]
async fn both_protocol_versions_preserve_messages_and_channels() {
    timeout(Duration::from_secs(10), async {
        for version in [10, 11] {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();
            listener.listen().await;
            let client = RaknetSocket::connect_with_version(&addr, version)
                .await
                .unwrap();
            let server = listener.accept().await.unwrap();
            assert_eq!(server.raknet_version().unwrap(), version);
            for (index, reliability) in [
                Reliability::Unreliable,
                Reliability::UnreliableSequenced,
                Reliability::Reliable,
                Reliability::ReliableOrdered,
                Reliability::ReliableSequenced,
            ]
            .into_iter()
            .enumerate()
            {
                let message = vec![0xfe, index as u8];
                client
                    .send_with_order_channel(&message, reliability, index as u8)
                    .await
                    .unwrap();
                assert_eq!(server.recv().await.unwrap(), message);
            }
            let mut message = vec![0xfe; 10_000];
            message[1..9].copy_from_slice(&123_u64.to_le_bytes());
            server
                .send_with_order_channel(&message, Reliability::ReliableOrdered, 7)
                .await
                .unwrap();
            assert_eq!(client.recv().await.unwrap(), message);
            client.close().await.unwrap();
            server.close().await.unwrap();
            listener.close().await.unwrap();
        }
    })
    .await
    .expect("protocol-version regression timed out");
}

#[tokio::test]
async fn closing_listener_unblocks_a_stalled_application_receiver() {
    timeout(Duration::from_secs(10), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        listener.listen().await;
        let client = RaknetSocket::connect(&addr).await.unwrap();
        let _server = listener.accept().await.unwrap();
        for _ in 0..1000 {
            client
                .send(&[0xfe], Reliability::ReliableOrdered)
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        listener.close().await.unwrap();
        client.close().await.unwrap();
    })
    .await
    .expect("shutdown blocked behind the full application receive queue");
}

#[tokio::test]
async fn cancelling_nethernet_proxy_closes_active_forwarders() {
    timeout(Duration::from_secs(5), async {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = NetherNetProxy::bind(
            "127.0.0.1:0".parse().unwrap(),
            upstream.local_addr().unwrap(),
        )
        .await
        .unwrap()
        .with_connection_limit(std::num::NonZeroUsize::new(1).unwrap());
        let address = proxy.local_addr().unwrap();
        let proxy_task = tokio::spawn(async move { proxy.run().await });
        let mut client = TcpStream::connect(address).await.unwrap();
        let (mut backend, _) = upstream.accept().await.unwrap();
        client.write_all(b"x").await.unwrap();
        let mut byte = [0];
        backend.read_exact(&mut byte).await.unwrap();
        proxy_task.abort();
        let _ = proxy_task.await;
        assert_eq!(client.read(&mut byte).await.unwrap(), 0);
        assert_eq!(backend.read(&mut byte).await.unwrap(), 0);
    })
    .await
    .expect("a forwarding task survived its proxy");
}

#[tokio::test]
async fn bidirectional_bursts_keep_processing_acks_under_backpressure() {
    timeout(Duration::from_secs(15), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        listener.listen().await;
        let client = RaknetSocket::connect(&address).await.unwrap();
        let server = listener.accept().await.unwrap();
        async fn exchange(socket: &RaknetSocket) {
            let send = async {
                for index in 0..5000_u32 {
                    let mut payload = [0xfe; 800];
                    payload[1..5].copy_from_slice(&index.to_le_bytes());
                    socket
                        .send(&payload, Reliability::ReliableOrdered)
                        .await
                        .unwrap();
                }
            };
            let receive = async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                for index in 0..5000_u32 {
                    let data = socket.recv().await.unwrap();
                    assert_eq!(&data[1..5], &index.to_le_bytes());
                }
            };
            tokio::join!(send, receive);
        }
        tokio::join!(exchange(&client), exchange(&server));
        client.close().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
    })
    .await
    .expect("application backpressure prevented ACK processing");
}

#[tokio::test]
async fn disconnect_during_handshake_does_not_leave_connect_waiting() {
    timeout(Duration::from_secs(3), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        listener.listen().await;
        let connecting = tokio::spawn(async move { RaknetSocket::connect(&address).await });
        let server = listener.accept().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
        assert!(connecting.await.unwrap().is_err());
    })
    .await
    .expect("connect ignored a disconnect during its connected handshake");
}

#[tokio::test]
async fn lost_offline_reply_is_replayed_without_replacing_the_session() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    timeout(Duration::from_secs(8), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let backend = listener.local_addr().unwrap();
        listener.listen().await;
        let relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = relay.local_addr().unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let observed = dropped.clone();
        let relay_task = tokio::spawn(async move {
            let mut client = None;
            let mut buf = [0; 2048];
            loop {
                let (len, source) = relay.recv_from(&mut buf).await.unwrap();
                if source == backend {
                    if buf[0] == 0x08 && !observed.swap(true, Ordering::Relaxed) {
                        continue;
                    }
                    relay.send_to(&buf[..len], client.unwrap()).await.unwrap();
                } else {
                    client = Some(source);
                    relay.send_to(&buf[..len], backend).await.unwrap();
                }
            }
        });
        let client = RaknetSocket::connect(&address).await.unwrap();
        let server = listener.accept().await.unwrap();
        assert!(dropped.load(Ordering::Relaxed));
        client
            .send(&[0xfe, 42], Reliability::ReliableOrdered)
            .await
            .unwrap();
        assert_eq!(server.recv().await.unwrap(), vec![0xfe, 42]);
        client.close().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
        relay_task.abort();
    })
    .await
    .expect("Request2 retransmission did not recover its lost reply");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_construction_and_drop_cannot_consume_its_own_startup_signal() {
    timeout(Duration::from_secs(10), async {
        for _ in 0..500 {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            listener.listen().await;
            drop(listener);
        }
        for _ in 0..20 {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            listener.listen().await;
            let client = RaknetSocket::connect(&address).await.unwrap();
            let server = listener.accept().await.unwrap();
            drop(server);
            assert!(
                timeout(Duration::from_secs(2), client.recv())
                    .await
                    .unwrap()
                    .is_err()
            );
            listener.close().await.unwrap();
        }
    })
    .await
    .expect("construction or Drop notification stalled");
}
