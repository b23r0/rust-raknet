#![cfg(feature = "blocking")]

use rust_raknet::{
    Bytes, Reliability,
    blocking::{Error, RaknetRuntime},
};
use std::{num::NonZeroUsize, sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(10);

fn pair() -> (
    rust_raknet::blocking::RaknetListener,
    rust_raknet::blocking::RaknetSocket,
    rust_raknet::blocking::RaknetSocket,
) {
    let runtime = RaknetRuntime::new().unwrap();
    let mut listener = runtime.bind(&"127.0.0.1:0".parse().unwrap()).unwrap();
    listener.listen().unwrap();
    let client = runtime
        .connect_timeout(&listener.local_addr().unwrap(), WAIT)
        .unwrap();
    let server = listener.accept_timeout(WAIT).unwrap();
    // The sockets and listener must keep workers alive after this owner drops.
    drop(runtime);
    (listener, client, server)
}

#[test]
fn all_delivery_modes_and_order_channels_round_trip() {
    let (mut listener, client, server) = pair();
    assert_eq!(
        client.raknet_version().unwrap(),
        server.raknet_version().unwrap()
    );
    assert_eq!(client.peer_addr().unwrap(), listener.local_addr().unwrap());
    for (index, mode) in [
        Reliability::Unreliable,
        Reliability::UnreliableSequenced,
        Reliability::Reliable,
        Reliability::ReliableOrdered,
        Reliability::ReliableSequenced,
    ]
    .into_iter()
    .enumerate()
    {
        let payload = [0xfe, index as u8];
        client
            .send_with_order_channel(&payload, mode, index as u8)
            .unwrap();
        assert_eq!(server.recv_timeout(WAIT).unwrap(), payload);
        server
            .send_bytes_with_order_channel(Bytes::copy_from_slice(&payload), mode, index as u8)
            .unwrap();
        assert_eq!(client.recv_bytes_timeout(WAIT).unwrap(), payload.as_slice());
    }
    client.flush_timeout(WAIT).unwrap();
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[test]
fn fragments_and_owned_batches_preserve_payloads() {
    let (mut listener, client, server) = pair();
    let payload = Bytes::from(vec![0xfe; 16_000]);
    client
        .send_bytes(payload.clone(), Reliability::ReliableOrdered)
        .unwrap();
    assert_eq!(server.recv_bytes_timeout(WAIT).unwrap(), payload);
    let messages = [
        Bytes::from_static(&[0xfe, 1]),
        Bytes::from_static(&[0xfe, 2]),
        Bytes::from_static(&[0xfe, 3]),
    ];
    server
        .send_bytes_batch_with_order_channel(&messages, Reliability::ReliableOrdered, 7)
        .unwrap();
    let mut received = Vec::new();
    while received.len() < messages.len() {
        let mut batch = Vec::new();
        client.recv_batch_timeout(&mut batch, 64, WAIT).unwrap();
        received.extend(batch.into_iter().map(Bytes::from));
    }
    assert_eq!(received, messages);
    client.flush_timeout(WAIT).unwrap();
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[test]
fn timed_out_receives_do_not_consume_later_messages() {
    let (mut listener, client, server) = pair();
    assert!(matches!(
        client.recv_timeout(Duration::from_millis(20)),
        Err(Error::Timeout)
    ));
    let mut batch = vec![vec![99]];
    assert!(matches!(
        client.recv_batch_timeout(&mut batch, 64, Duration::from_millis(20)),
        Err(Error::Timeout)
    ));
    assert!(batch.is_empty());
    server
        .send(&[0xfe, 42], Reliability::ReliableOrdered)
        .unwrap();
    assert_eq!(client.recv_timeout(WAIT).unwrap(), [0xfe, 42]);
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[test]
fn accept_timeout_leaves_listener_usable_and_motd_is_discoverable() {
    let runtime = RaknetRuntime::new().unwrap();
    let mut listener = runtime
        .bind(&"127.0.0.1:0".parse().unwrap())
        .unwrap()
        .with_accept_backlog(NonZeroUsize::new(4).unwrap());
    listener.set_full_motd("blocking discovery".into()).unwrap();
    assert_eq!(listener.get_motd().unwrap(), "blocking discovery");
    listener.listen().unwrap();
    assert!(matches!(
        listener.accept_timeout(Duration::from_millis(20)),
        Err(Error::Timeout)
    ));
    let address = listener.local_addr().unwrap();
    assert_eq!(
        runtime.ping_timeout(&address, WAIT).unwrap().1,
        "blocking discovery"
    );
    let client = runtime
        .connect_with_version_and_mtu(&address, 11, 576)
        .unwrap();
    let server = listener.accept_timeout(WAIT).unwrap();
    assert_eq!(client.mtu(), 576);
    assert_eq!(
        listener
            .get_peer_raknet_version(&server.peer_addr().unwrap())
            .unwrap(),
        11
    );
    server.send(&[0xfe], Reliability::ReliableOrdered).unwrap();
    assert_eq!(client.recv_timeout(WAIT).unwrap(), [0xfe]);
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[test]
fn runtime_drives_acknowledgements_between_blocking_calls() {
    let (mut listener, client, server) = pair();
    client
        .send(&[0xfe, 7], Reliability::ReliableOrdered)
        .unwrap();
    // No API call drives either runtime during this interval.
    std::thread::sleep(Duration::from_millis(100));
    client.flush_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(server.recv_timeout(WAIT).unwrap(), [0xfe, 7]);
    listener.close().unwrap();
}

#[test]
fn concurrent_receive_does_not_block_sending_on_the_same_socket() {
    let (mut listener, client, server) = pair();
    let client = Arc::new(client);
    let receiver = client.clone();
    let waiting = std::thread::spawn(move || receiver.recv_timeout(WAIT).unwrap());
    client
        .send(&[0xfe, 1], Reliability::ReliableOrdered)
        .unwrap();
    assert_eq!(server.recv_timeout(WAIT).unwrap(), [0xfe, 1]);
    server
        .send(&[0xfe, 2], Reliability::ReliableOrdered)
        .unwrap();
    assert_eq!(waiting.join().unwrap(), [0xfe, 2]);
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[test]
fn remote_close_unblocks_blocking_receive() {
    let (mut listener, client, server) = pair();
    server.close().unwrap();
    assert!(matches!(
        client.recv_timeout(WAIT),
        Err(Error::Raknet(
            rust_raknet::error::RaknetError::ConnectionClosed
        ))
    ));
    client.close().unwrap();
    client.close().unwrap();
    listener.close().unwrap();
}

#[test]
fn unreachable_peer_has_an_overall_connect_deadline() {
    let runtime = RaknetRuntime::new().unwrap();
    let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert!(matches!(
        runtime.connect_timeout(&blackhole.local_addr().unwrap(), Duration::from_millis(30)),
        Err(Error::Timeout)
    ));
}

#[tokio::test]
async fn runtime_creation_in_async_context_returns_an_error() {
    assert!(matches!(RaknetRuntime::new(), Err(Error::AsyncContext)));
}

#[test]
fn blocking_calls_reject_async_context_and_last_owner_can_drop_there() {
    let (mut listener, client, server) = pair();
    let runtime = RaknetRuntime::new().unwrap();
    let async_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    async_runtime.block_on(async {
        assert!(matches!(
            client.recv_timeout(Duration::from_millis(1)),
            Err(Error::AsyncContext)
        ));
        assert!(matches!(
            client.send(&[0xfe], Reliability::ReliableOrdered),
            Err(Error::AsyncContext)
        ));
        assert!(matches!(
            runtime.connect(&listener.local_addr().unwrap()),
            Err(Error::AsyncContext)
        ));
        drop(runtime);
    });
    server
        .send(&[0xfe, 9], Reliability::ReliableOrdered)
        .unwrap();
    assert_eq!(client.recv_timeout(WAIT).unwrap(), [0xfe, 9]);
    server.flush_timeout(WAIT).unwrap();
    listener.close().unwrap();
}

#[cfg(feature = "send-policy")]
#[test]
fn send_policy_is_available_from_the_blocking_socket() {
    let (mut listener, client, _server) = pair();
    client
        .set_send_options(rust_raknet::SendOptions::default())
        .unwrap();
    assert!(client.send_options().unwrap().is_some());
    listener.close().unwrap();
}

#[cfg(feature = "recovery-policy")]
#[test]
fn recovery_policy_is_available_from_the_blocking_socket() {
    let (mut listener, client, _server) = pair();
    let options = rust_raknet::RecoveryOptions {
        deadline_driven: true,
        tail_probe_min_delay: Some(Duration::from_millis(10)),
        reset_backoff_on_progress: true,
    };
    client.set_recovery_options(options).unwrap();
    assert_eq!(client.recovery_options().unwrap(), options);
    listener.close().unwrap();
}
