use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use std::time::Duration;
use tokio::{net::UdpSocket, time::timeout};

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
    timeout(Duration::from_secs(60), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap()
            .with_accept_backlog(std::num::NonZeroUsize::new(4).unwrap());
        let address = listener.local_addr().unwrap();
        listener.listen().await;

        // Complete four handshakes without accepting any sockets. The backlog
        // is now full regardless of the runner's timer resolution or load.
        let mut initial_clients = Vec::new();
        for index in 0..4_u8 {
            let client = timeout(
                Duration::from_secs(10),
                RaknetSocket::connect_with_version(&address, 11),
            )
            .await
            .expect("initial handshake did not fill the accept backlog")
            .unwrap();
            initial_clients.push((index, client));
        }

        let (replies_acknowledged, _) = tokio::sync::watch::channel(false);
        let mut clients = tokio::task::JoinSet::new();
        for index in 0..16_u8 {
            let initial = if index < 4 {
                Some(initial_clients.remove(0).1)
            } else {
                None
            };
            let mut acknowledged = replies_acknowledged.subscribe();
            clients.spawn(async move {
                let client = match initial {
                    Some(client) => client,
                    None => timeout(
                        Duration::from_secs(15),
                        RaknetSocket::connect_with_version(&address, 11),
                    )
                    .await
                    .unwrap_or_else(|_| panic!("backlogged client {index} did not connect"))
                    .unwrap(),
                };
                timeout(Duration::from_secs(10), async {
                    client
                        .send(&[0xfe, index], Reliability::ReliableOrdered)
                        .await
                        .unwrap();
                    assert_eq!(client.recv().await.unwrap(), vec![0xfe, index]);
                })
                .await
                .unwrap_or_else(|_| panic!("client {index} did not receive its echo"));

                // Keep clients alive until every server has received its ACK.
                // Remote disconnect delivery is covered separately; it is not
                // a prerequisite for proving backlog recovery.
                acknowledged.wait_for(|done| *done).await.unwrap();
                client.close().await.unwrap();
            });
        }
        // Give overflow handshakes a chance to exercise Request2 retry while
        // the already-filled backlog remains untouched.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut servers = tokio::task::JoinSet::new();
        for index in 0..16 {
            let server = timeout(Duration::from_secs(15), listener.accept())
                .await
                .unwrap_or_else(|_| panic!("accept {index} did not recover after draining backlog"))
                .unwrap();
            servers.spawn(async move {
                timeout(Duration::from_secs(10), async {
                    let packet = server.recv().await.unwrap();
                    server
                        .send(&packet, Reliability::ReliableOrdered)
                        .await
                        .unwrap();
                    server.flush().await.unwrap();
                })
                .await
                .unwrap_or_else(|_| panic!("accepted socket {index} did not finish echo and ACK"));
                server
            });
        }
        // Keep server sockets alive too, so closing a fast peer cannot race a
        // slower client's echo receive while other handshakes are retrying.
        let mut completed_servers = Vec::new();
        while let Some(result) = servers.join_next().await {
            completed_servers.push(result.unwrap());
        }
        replies_acknowledged.send(true).unwrap();
        while let Some(result) = clients.join_next().await {
            result.unwrap();
        }
        for server in completed_servers {
            server.close().await.unwrap();
        }
        listener.close().await.unwrap();
    })
    .await
    .expect("accept backlog saturation test did not complete");
}

#[tokio::test]
async fn batches_preserve_order_channels_fragments_and_recover_from_loss() {
    timeout(Duration::from_secs(20), async {
        for version in [10, 11] {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            let addr = listener.local_addr().unwrap();
            listener.listen().await;
            let mut client = RaknetSocket::connect_with_version(&addr, version)
                .await
                .unwrap();
            let mut server = listener.accept().await.unwrap();
            client.set_loss_rate(2);
            server.set_loss_rate(2);
            let messages: Vec<_> = (0..160u16)
                .map(|index| {
                    let mut message = vec![0xfe; if index % 17 == 0 { 4096 } else { 64 }];
                    message[1..3].copy_from_slice(&index.to_le_bytes());
                    rust_raknet::Bytes::from(message)
                })
                .collect();
            let send = client.send_bytes_batch_with_order_channel(
                &messages,
                Reliability::ReliableOrdered,
                7,
            );
            let receive = async {
                let mut offset = 0;
                let mut batch = Vec::with_capacity(16);
                while offset < messages.len() {
                    server.recv_bytes_batch(&mut batch, 16).await.unwrap();
                    for actual in &batch {
                        assert_eq!(actual, &messages[offset]);
                        offset += 1;
                    }
                }
            };
            let (result, ()) = tokio::join!(send, receive);
            result.unwrap();
            client.flush().await.unwrap();
            client.set_loss_rate(0);
            server.set_loss_rate(0);
            for mode in [
                Reliability::Unreliable,
                Reliability::UnreliableSequenced,
                Reliability::Reliable,
                Reliability::ReliableOrdered,
                Reliability::ReliableSequenced,
            ] {
                let messages: [&[u8]; 2] = [&[0xfe, 1], &[0xfe, 2]];
                server
                    .send_batch_with_order_channel(&messages, mode, 3)
                    .await
                    .unwrap();
                assert_eq!(client.recv().await.unwrap(), messages[0]);
                assert_eq!(client.recv().await.unwrap(), messages[1]);
            }
            client.close().await.unwrap();
            server.close().await.unwrap();
            listener.close().await.unwrap();
        }
    })
    .await
    .expect("batched reliable delivery stalled");
}

#[tokio::test]
async fn batch_validation_is_atomic_and_receiving_never_waits_to_fill() {
    timeout(Duration::from_secs(5), async {
        let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        listener.listen().await;
        let client = RaknetSocket::connect(&addr).await.unwrap();
        let server = listener.accept().await.unwrap();
        assert!(
            client
                .send_batch(&[&[0xfe, 1], &[]], Reliability::ReliableOrdered)
                .await
                .is_err()
        );
        let oversized = vec![0xfe; 2000];
        assert!(
            client
                .send_batch(&[&[0xfe, 2], &oversized], Reliability::Reliable)
                .await
                .is_err()
        );
        client
            .send_batch(&[], Reliability::ReliableOrdered)
            .await
            .unwrap();
        let mut messages = Vec::new();
        assert_eq!(server.recv_batch(&mut messages, 0).await.unwrap(), 0);
        client
            .send(&[0xfe, 3], Reliability::ReliableOrdered)
            .await
            .unwrap();
        assert_eq!(server.recv_batch(&mut messages, 64).await.unwrap(), 1);
        assert_eq!(messages, vec![vec![0xfe, 3]]);
        assert!(
            timeout(Duration::from_millis(30), server.recv())
                .await
                .is_err()
        );
        client.close().await.unwrap();
        server.close().await.unwrap();
        listener.close().await.unwrap();
    })
    .await
    .expect("batch validation or sparse receive stalled");
}
