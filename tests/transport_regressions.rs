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

#[tokio::test]
async fn final_mtu_negotiation_controls_fragmentation() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    timeout(Duration::from_secs(10), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let backend = listener.local_addr().unwrap();
        listener.listen().await;
        let relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = relay.local_addr().unwrap();
        let largest = Arc::new(AtomicUsize::new(0));
        let observed = largest.clone();
        let relay_task = tokio::spawn(async move {
            let mut client = None;
            let mut buf = [0; 2048];
            loop {
                let (len, source) = relay.recv_from(&mut buf).await.unwrap();
                if source == backend {
                    if buf[0] == 0x08 {
                        // Reply2 ends with the final MTU followed by its encryption flag.
                        buf[len - 3..len - 1].copy_from_slice(&576_u16.to_be_bytes());
                    }
                    relay.send_to(&buf[..len], client.unwrap()).await.unwrap();
                } else {
                    client = Some(source);
                    if (0x80..=0x8d).contains(&buf[0]) {
                        observed.fetch_max(len, Ordering::Relaxed);
                    }
                    relay.send_to(&buf[..len], backend).await.unwrap();
                }
            }
        });
        let client = RaknetSocket::connect_with_version(&address, 11)
            .await
            .unwrap();
        let server = listener.accept().await.unwrap();
        let payload = vec![0xfe; 4096];
        client
            .send(&payload, Reliability::ReliableOrdered)
            .await
            .unwrap();
        assert_eq!(server.recv().await.unwrap(), payload);
        client.flush().await.unwrap();
        assert!(largest.load(Ordering::Relaxed) <= 576);
        client.close().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
        relay_task.abort();
    })
    .await
    .expect("the client ignored the final negotiated MTU");
}

#[tokio::test]
async fn concurrent_flush_waiters_wake_on_ack_and_close() {
    use std::sync::Arc;
    timeout(Duration::from_secs(10), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        listener.listen().await;
        let client = RaknetSocket::connect(&address).await.unwrap();
        let mut server = listener.accept().await.unwrap();
        server.set_loss_rate(10);
        client
            .send(&[0xfe, 1], Reliability::ReliableOrdered)
            .await
            .unwrap();
        let client = Arc::new(client);
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let client = client.clone();
            waiters.spawn(async move { client.flush().await });
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            waiters.try_join_next().is_none(),
            "flush completed before an ACK"
        );
        server.set_loss_rate(0);
        assert_eq!(server.recv().await.unwrap(), vec![0xfe, 1]);
        while let Some(result) = waiters.join_next().await {
            result.unwrap().unwrap();
        }
        server.set_loss_rate(10);
        client
            .send(&[0xfe, 2], Reliability::ReliableOrdered)
            .await
            .unwrap();
        let waiting = client.flush();
        tokio::pin!(waiting);
        assert!(
            timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        client.close().await.unwrap();
        assert!(waiting.await.is_err());
        server.close().await.unwrap();
        listener.close().await.unwrap();
    })
    .await
    .expect("concurrent flush waiters were not notified");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_accept_backlog_recovers_without_disconnecting_handshakes() {
    timeout(Duration::from_secs(12), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap()
            .with_accept_backlog(std::num::NonZeroUsize::new(4).unwrap());
        let address = listener.local_addr().unwrap();
        listener.listen().await;
        let mut clients = tokio::task::JoinSet::new();
        for index in 0..16_u8 {
            clients.spawn(async move {
                let client = RaknetSocket::connect_with_version(&address, 11)
                    .await
                    .unwrap();
                client
                    .send(&[0xfe, index], Reliability::ReliableOrdered)
                    .await
                    .unwrap();
                assert_eq!(client.recv().await.unwrap(), vec![0xfe, index]);
                // Keep the peer open until the server's flush completes. Closing
                // first can legitimately cancel flush while the ACK is in flight.
                assert!(client.recv().await.is_err());
                client.close().await.unwrap();
            });
        }
        // Let the configured four-entry backlog fill before the application accepts.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut servers = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let server = listener.accept().await.unwrap();
            servers.spawn(async move {
                let packet = server.recv().await.unwrap();
                server
                    .send(&packet, Reliability::ReliableOrdered)
                    .await
                    .unwrap();
                server.flush().await.unwrap();
                let _ = server.close().await;
            });
        }
        while let Some(result) = clients.join_next().await {
            result.unwrap();
        }
        while let Some(result) = servers.join_next().await {
            result.unwrap();
        }
        listener.close().await.unwrap();
    })
    .await
    .expect("accept backlog saturation broke an offline handshake");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nethernet_concurrent_connections_obey_the_configured_limit() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    timeout(Duration::from_secs(10), async {
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = backend.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let observed = peak.clone();
        let backend_task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..64 {
                let (mut socket, _) = backend.accept().await.unwrap();
                let active = active.clone();
                let peak = observed.clone();
                tasks.spawn(async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    let mut request = Vec::new();
                    socket.read_to_end(&mut request).await.unwrap();
                    socket.write_all(&request).await.unwrap();
                    socket.shutdown().await.unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        });
        let proxy = NetherNetProxy::bind("127.0.0.1:0".parse().unwrap(), address)
            .await
            .unwrap()
            .with_connection_limit(std::num::NonZeroUsize::new(8).unwrap());
        let proxy_address = proxy.local_addr().unwrap();
        let proxy_task = tokio::spawn(async move { proxy.run().await });
        let mut clients = tokio::task::JoinSet::new();
        for index in 0..64_u8 {
            clients.spawn(async move {
                let mut socket = TcpStream::connect(proxy_address).await.unwrap();
                let payload = vec![index; 8192];
                socket.write_all(&payload).await.unwrap();
                socket.shutdown().await.unwrap();
                let mut response = Vec::new();
                socket.read_to_end(&mut response).await.unwrap();
                assert_eq!(response, payload);
            });
        }
        while let Some(result) = clients.join_next().await {
            result.unwrap();
        }
        backend_task.await.unwrap();
        assert!((1..=8).contains(&peak.load(Ordering::SeqCst)));
        proxy_task.abort();
        let _ = proxy_task.await;
    })
    .await
    .expect("concurrent NetherNet signaling stalled or bypassed its limit");
}
