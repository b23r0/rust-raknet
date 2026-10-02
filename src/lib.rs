//! An asynchronous implementation of the RakNet transport protocol in Rust.
//!
//! RakNet provides reliable and ordered delivery modes over UDP and is used by
//! Minecraft Bedrock Edition. This library implements the connection handshake,
//! packet reliability, fragmentation, and listener/client APIs.
//!
//! The library requires the Tokio 1.x asynchronous runtime.
//!
//! # Features
//!
//! - Asynchronous client and server APIs
//! - Five RakNet reliability modes
//! - Selective retransmission and fast acknowledgements
//! - IPv4 and IPv6 socket addresses
//!
//! # Getting started
//!
//! ```toml
//! [dependencies]
//! rust-raknet = "0.16.0"
//! ```
//!
//! # Reliability modes
//!
//! - Unreliable
//! - Unreliable sequenced
//! - Reliable
//! - Reliable ordered
//! - Reliable sequenced
//!
//! This project is not affiliated with Jenkins Software LLC or RakNet.
//! See the [RakNet protocol reference](http://www.jenkinssoftware.com/raknet/manual/index.html).

mod arq;
mod datatype;
pub mod error;
mod fragment;
mod log;
mod packet;
mod sequence;
mod server;
mod socket;
mod udp;
mod utils;

pub use crate::arq::Reliability;
pub use crate::log::enable_raknet_log;
pub use crate::server::*;
pub use crate::socket::*;
pub use bytes::Bytes;

#[test]
fn test_raknet_error_implements_std_error() {
    fn assert_error<T: std::error::Error>() {}
    assert_error::<error::RaknetError>();
    assert_eq!(
        error::RaknetError::NotListen.to_string(),
        "the RakNet listener is not listening"
    );
}

#[tokio::test]
async fn test_listener_updates_motd_after_listen() {
    let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = listener.local_addr().unwrap();
    listener.listen().await;

    let motd = "MCPE;Updated MOTD;486;1.20.0;0;20;123;World;Survival;1;19132;";
    listener.set_full_motd(motd.to_owned()).await.unwrap();

    let (_, received_motd) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        RaknetSocket::ping(&local_addr),
    )
    .await
    .expect("server did not answer the unconnected ping")
    .unwrap();
    assert_eq!(received_motd, motd);

    listener.close().await.unwrap();
}

#[tokio::test]
async fn test_listener_accepts_more_than_one_queue_of_connections() {
    const CONNECTIONS: usize = 12;

    let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = listener.local_addr().unwrap();
    listener.listen().await;

    let accept_task = tokio::spawn(async move {
        let mut accepted = Vec::with_capacity(CONNECTIONS);
        for _ in 0..CONNECTIONS {
            accepted.push(
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .expect("listener stopped accepting connections")
                    .unwrap(),
            );
        }
        (accepted, listener)
    });

    let mut clients = Vec::with_capacity(CONNECTIONS);
    for _ in 0..CONNECTIONS {
        clients.push(
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                RaknetSocket::connect(&local_addr),
            )
            .await
            .expect("client connection timed out")
            .unwrap(),
        );
    }

    let (accepted, mut listener) =
        tokio::time::timeout(std::time::Duration::from_secs(5), accept_task)
            .await
            .expect("listener accept loop timed out")
            .unwrap();
    assert_eq!(accepted.len(), CONNECTIONS);

    for socket in clients {
        socket.close().await.unwrap();
    }
    for socket in accepted {
        socket.close().await.unwrap();
    }
    listener.close().await.unwrap();
}

#[tokio::test]
async fn test_connect() {
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;

    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();

    tokio::spawn(async move {
        let client1 = server.accept().await.unwrap();
        assert!(client1.local_addr().unwrap() == local_addr);
        client1
            .send(&[0xfe, 2, 3], Reliability::Reliable)
            .await
            .unwrap();
        notify2.notified().await;
    });
    let client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    assert!(client2.peer_addr().unwrap() == local_addr);
    let buf = client2.recv().await.unwrap();
    assert!(buf == vec![0xfe, 2, 3]);

    notify.notify_one();
}

#[tokio::test]
async fn test_send_recv_fragment_data() {
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;

    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();

    tokio::spawn(async move {
        let client1 = server.accept().await.unwrap();
        assert!(client1.local_addr().unwrap() == local_addr);

        let mut a = vec![3u8; 1000];
        let mut b = vec![2u8; 1000];
        let mut c = vec![0xfe; 1000];
        b.append(&mut a);
        c.append(&mut b);

        client1
            .send(&c, Reliability::ReliableOrdered)
            .await
            .unwrap();

        notify2.notified().await;
    });
    let client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    assert!(client2.peer_addr().unwrap() == local_addr);
    let buf = client2.recv().await.unwrap();
    assert!(buf.len() == 3000);
    assert!(buf[0..1000] == vec![0xfe; 1000]);
    assert!(buf[1000..2000] == vec![2u8; 1000]);
    assert!(buf[2000..3000] == vec![3u8; 1000]);

    notify.notify_one();
}

#[tokio::test]
async fn test_send_recv_more_reliability_type_packet() {
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;

    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();

    tokio::spawn(async move {
        let client1 = server.accept().await.unwrap();
        assert!(client1.local_addr().unwrap() == local_addr);

        client1
            .send(&[0xfe, 1, 2, 3], Reliability::Unreliable)
            .await
            .unwrap();
        let data = client1.recv().await.unwrap();
        assert!(data == [0xfe, 4, 5, 6].to_vec());

        client1
            .send(&[0xfe, 7, 8, 9], Reliability::UnreliableSequenced)
            .await
            .unwrap();
        let data = client1.recv().await.unwrap();
        assert!(data == [0xfe, 10, 11, 12].to_vec());

        client1
            .send(&[0xfe, 13, 14, 15], Reliability::Reliable)
            .await
            .unwrap();
        let data = client1.recv().await.unwrap();
        assert!(data == [0xfe, 16, 17, 18].to_vec());

        let mut a = vec![3u8; 1000];
        let mut b = vec![2u8; 1000];
        let mut c = vec![0xfe; 1000];
        b.append(&mut a);
        c.append(&mut b);

        client1
            .send(&c, Reliability::ReliableOrdered)
            .await
            .unwrap();

        let buf = client1.recv().await.unwrap();
        assert!(buf.len() == 3000);
        assert!(buf[0..1000] == vec![0xfe; 1000]);
        assert!(buf[1000..2000] == vec![2u8; 1000]);
        assert!(buf[2000..3000] == vec![3u8; 1000]);

        client1
            .send(&[0xfe, 19, 20, 21], Reliability::ReliableSequenced)
            .await
            .unwrap();
        let data = client1.recv().await.unwrap();
        assert!(data == [0xfe, 22, 23, 24].to_vec());

        notify2.notified().await;
    });
    let client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    assert!(client2.peer_addr().unwrap() == local_addr);

    let buf = client2.recv().await.unwrap();
    assert!(buf == [0xfe, 1, 2, 3]);

    client2
        .send(&[0xfe, 4, 5, 6], Reliability::Unreliable)
        .await
        .unwrap();

    let buf = client2.recv().await.unwrap();
    assert!(buf == [0xfe, 7, 8, 9]);

    client2
        .send(&[0xfe, 10, 11, 12], Reliability::UnreliableSequenced)
        .await
        .unwrap();

    let buf = client2.recv().await.unwrap();
    assert!(buf == [0xfe, 13, 14, 15]);

    client2
        .send(&[0xfe, 16, 17, 18], Reliability::Reliable)
        .await
        .unwrap();

    let buf = client2.recv().await.unwrap();
    assert!(buf.len() == 3000);
    assert!(buf[0..1000] == vec![0xfe; 1000]);
    assert!(buf[1000..2000] == vec![2u8; 1000]);
    assert!(buf[2000..3000] == vec![3u8; 1000]);

    let mut a = vec![3u8; 1000];
    let mut b = vec![2u8; 1000];
    let mut c = vec![0xfe; 1000];
    b.append(&mut a);
    c.append(&mut b);

    client2
        .send(&c, Reliability::ReliableOrdered)
        .await
        .unwrap();

    let buf = client2.recv().await.unwrap();
    assert!(buf == [0xfe, 19, 20, 21]);

    client2
        .send(&[0xfe, 22, 23, 24], Reliability::ReliableSequenced)
        .await
        .unwrap();

    notify.notify_one();
}

#[tokio::test]
async fn test_loss_packet1() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;
    tokio::spawn(async move {
        let mut client1 = server.accept().await.unwrap();
        // Simulate 20% packet loss.
        client1.set_loss_rate(2);

        for i in 0..10 {
            let mut flag = vec![0xfe_u8];
            let mut data = vec![i as u8; 2000];
            flag.append(&mut data);
            client1
                .send(&flag, Reliability::ReliableOrdered)
                .await
                .unwrap();

            let data = client1.recv().await.unwrap();
            assert!(data == flag);
        }

        notify2.notified().await;
    });
    let mut client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    // Simulate 20% packet loss.
    client2.set_loss_rate(2);

    for i in 0..10 {
        let mut flag = vec![0xfe_u8];
        let mut data = vec![i as u8; 2000];
        flag.append(&mut data);
        client2
            .send(&flag, Reliability::ReliableOrdered)
            .await
            .unwrap();

        let data = client2.recv().await.unwrap();
        assert!(data == flag);
    }
    notify.notify_one();
}

#[tokio::test]
async fn test_loss_packet2() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;
    tokio::spawn(async move {
        let mut client1 = server.accept().await.unwrap();
        // Simulate 20% packet loss.
        client1.set_loss_rate(2);

        for i in 0..10 {
            let mut flag = vec![0xfe_u8];
            let mut data = vec![i as u8; 2000];
            flag.append(&mut data);
            client1
                .send(&flag, Reliability::ReliableOrdered)
                .await
                .unwrap();
        }

        for i in 0..10 {
            let mut flag = vec![0xfe_u8];
            let mut data = vec![i as u8; 2000];
            flag.append(&mut data);
            let data = client1.recv().await.unwrap();
            assert!(data == flag);
        }
        notify2.notified().await;
    });
    let mut client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    // Simulate 20% packet loss.
    client2.set_loss_rate(2);

    for i in 0..10 {
        let mut flag = vec![0xfe_u8];
        let mut data = vec![i as u8; 2000];
        flag.append(&mut data);
        client2
            .send(&flag, Reliability::ReliableOrdered)
            .await
            .unwrap();
    }

    for i in 0..10 {
        let mut flag = vec![0xfe_u8];
        let mut data = vec![i as u8; 2000];
        flag.append(&mut data);
        let data = client2.recv().await.unwrap();
        assert!(data == flag);
    }
    notify.notify_one();
}

#[tokio::test]
async fn test_loss_packet_with_sequenced() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let local_addr = server.local_addr().unwrap();
    server.listen().await;
    tokio::spawn(async move {
        let mut client1 = server.accept().await.unwrap();
        // Simulate 20% packet loss.
        client1.set_loss_rate(2);

        for i in 0..100 {
            let mut flag = vec![0xfe_u8];
            let mut data = vec![i as u8; 20];
            flag.append(&mut data);
            client1
                .send(&flag, Reliability::ReliableSequenced)
                .await
                .unwrap();
        }

        let mut last = 0;
        for i in 0..50 {
            let mut flag = vec![0xfe_u8];
            let mut data = vec![i as u8; 20];
            flag.append(&mut data);
            let data = client1.recv().await.unwrap();
            assert!(data[1] >= last);
            last = data[1];
        }
        notify2.notified().await;
    });
    let mut client2 = RaknetSocket::connect(&local_addr).await.unwrap();
    // Simulate 20% packet loss.
    client2.set_loss_rate(2);

    for i in 0..100 {
        let mut flag = vec![0xfe_u8];
        let mut data = vec![i as u8; 20];
        flag.append(&mut data);
        client2
            .send(&flag, Reliability::ReliableSequenced)
            .await
            .unwrap();
    }

    let mut last = 0;
    for i in 0..50 {
        let mut flag = vec![0xfe_u8];
        let mut data = vec![i as u8; 20];
        flag.append(&mut data);
        let data = client2.recv().await.unwrap();
        assert!(data[1] >= last);
        last = data[1];
    }
    notify.notify_one();
}

#[tokio::test]
async fn test_raknet_server_close() {
    for _ in 0..10 {
        let mut server = RaknetListener::bind(&"127.0.0.1:19132".parse().unwrap())
            .await
            .unwrap();
        server.listen().await;
        let client = RaknetSocket::connect(&"127.0.0.1:19132".parse().unwrap())
            .await
            .unwrap();
        let mut a = vec![3u8; 1000];
        let mut b = vec![2u8; 1000];
        let mut c = vec![0xfe; 1000];
        b.append(&mut a);
        c.append(&mut b);
        client.send(&c, Reliability::ReliableOrdered).await.unwrap();

        let client2 = server.accept().await.unwrap();
        let buf = client2.recv().await.unwrap();
        assert!(buf.len() == 3000);
        assert!(buf[0..1000] == vec![0xfe; 1000]);
        assert!(buf[1000..2000] == vec![2u8; 1000]);
        assert!(buf[2000..3000] == vec![3u8; 1000]);

        server.close().await.unwrap();
        client.close().await.unwrap();
    }

    let mut server1 = RaknetListener::bind(&"127.0.0.1:19132".parse().unwrap())
        .await
        .unwrap();
    server1.listen().await;
    server1.close().await.unwrap();
    let mut server2 = RaknetListener::bind(&"127.0.0.1:19132".parse().unwrap())
        .await
        .unwrap();
    server2.listen().await;
}

#[tokio::test]
async fn test_send_recv_full_packet() {
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let remote_addr = format!("127.0.0.1:{}", server.local_addr().unwrap().port());
    tokio::spawn(async move {
        server.listen().await;
        let client = server.accept().await.unwrap();

        // Complete delivery requires ordered reliability; sequenced mode may
        // discard superseded messages during retransmission.
        for _ in 0..50 {
            client
                .send(&vec![0xfe; 1000], Reliability::ReliableOrdered)
                .await
                .unwrap();
        }
        client.flush().await.unwrap();

        client.close().await.unwrap();
        server.close().await.unwrap();
    });

    let client = RaknetSocket::connect(&remote_addr.parse().unwrap())
        .await
        .unwrap();

    for _ in 0..50 {
        let buf = client.recv().await.unwrap();
        assert!(buf == [0xfe; 1000]);
    }
}

#[tokio::test]
async fn test_send_recv_with_flush() {
    let mut server = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let remote_addr = format!("127.0.0.1:{}", server.local_addr().unwrap().port());
    let s1 = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let s2 = s1.clone();
    tokio::spawn(async move {
        server.listen().await;
        let client = server.accept().await.unwrap();

        for _ in 0..50 {
            #[allow(unused_must_use)]
            {
                s1.acquire().await.unwrap();
            }
            client
                .send(&vec![0xfe; 1000], Reliability::ReliableSequenced)
                .await
                .unwrap();
            client.flush().await.unwrap();
        }

        client.close().await.unwrap();
        for _ in 0..5 {
            client
                .send(&vec![0xfe; 1000], Reliability::ReliableSequenced)
                .await
                .unwrap();
            match match client.flush().await {
                Ok(_) => panic!("incorrect return"),
                Err(e) => e,
            } {
                error::RaknetError::ConnectionClosed => {}
                _ => panic!("incorrect return"),
            };
        }
        server.close().await.unwrap();
    });

    let client = RaknetSocket::connect(&remote_addr.parse().unwrap())
        .await
        .unwrap();

    for _ in 0..50 {
        let buf = client.recv().await.unwrap();
        assert!(buf == [0xfe; 1000]);
        s2.add_permits(1);
    }
}

/*
#[tokio::test]
async fn chore2(){

    enbale_raknet_log(true);
    let mut listener = RaknetListener::bind("0.0.0.0:19199".parse().unwrap()).await.unwrap();
    listener.listen().await;
    loop{
        let mut client1 = listener.accept().await.unwrap();
        let mut client2 = RaknetSocket::connect(&"192.168.199.127:19132".parse().unwrap()).await.unwrap();
        tokio::spawn(async move {
            println!("build connection");
            loop{
                tokio::select!{
                    a = client1.recv() => {
                        let a = match a{
                            Ok(p) => p,
                            Err(_) => {
                                client2.close().await.unwrap();
                                break;
                            },
                        };
                        match client2.send(&a, Reliability::ReliableOrdered).await{
                            Ok(p) => p,
                            Err(_) => {
                                client1.close().await.unwrap();
                                break;
                            },
                        };
                    },
                    b = client2.recv() => {
                        let b = match b{
                            Ok(p) => p,
                            Err(_) => {
                                client1.close().await.unwrap();
                                break;
                            },
                        };
                        match client1.send(&b, Reliability::ReliableOrdered).await{
                            Ok(p) => p,
                            Err(_) => {
                                client2.close().await.unwrap();
                                break;
                            },
                        };
                    }
                }
            }
            println!("close connection");
        });
    }


}
*/
