//! Real-time impairment tests; never run these on the host network.
use rand::{Rng, SeedableRng, rngs::StdRng};
use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    net::UdpSocket,
    sync::oneshot,
    time::{sleep, timeout},
};

#[derive(Default)]
struct Damage {
    enabled: AtomicBool,
    seen: AtomicU64,
    dropped: AtomicU64,
    duplicated: AtomicU64,
    delayed_acks: AtomicU64,
    peak_events: AtomicU64,
}
struct Datagram {
    upstream: bool,
    target: SocketAddr,
    wire: Vec<u8>,
}

async fn relay(
    front: UdpSocket,
    back: UdpSocket,
    target: SocketAddr,
    damage: Arc<Damage>,
    seed: u64,
) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut client = None;
    let mut events: BTreeMap<tokio::time::Instant, Vec<Datagram>> = BTreeMap::new();
    let mut a = [0_u8; 2048];
    let mut b = [0_u8; 2048];
    let start = Instant::now();
    loop {
        let wake = events
            .first_key_value()
            .map(|(due, _)| *due)
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(1));
        let (upstream, destination, wire) = tokio::select! {
            result=front.recv_from(&mut a)=> {
                let (n,peer)=result.unwrap();client=Some(peer);(true,target,a[..n].to_vec())
            }
            result=back.recv_from(&mut b)=> {
                let (n,peer)=result.unwrap();assert_eq!(peer,target);
                let Some(destination)=client else {continue};(false,destination,b[..n].to_vec())
            }
            _=tokio::time::sleep_until(wake)=> {
                while events.first_key_value().is_some_and(|(due,_)|*due<=tokio::time::Instant::now()) {
                    let (_,packets)=events.pop_first().unwrap();
                    for packet in packets {
                        let sock=if packet.upstream {&back}else{&front};
                        sock.send_to(&packet.wire,packet.target).await.unwrap();
                    }
                }
                continue;
            }
        };
        damage.seen.fetch_add(1, Ordering::Relaxed);
        let active = damage.enabled.load(Ordering::Acquire);
        let blackout = active && start.elapsed().as_secs() % 30 >= 28;
        if blackout || (active && rng.gen_ratio(35, 100)) {
            damage.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let delay = if !active {
            0
        } else if wire[0] == 0xc0 {
            damage.delayed_acks.fetch_add(1, Ordering::Relaxed);
            rng.gen_range(400..=2500)
        } else {
            rng.gen_range(0..=150)
        };
        let now = tokio::time::Instant::now();
        if active && rng.gen_ratio(1, 5) {
            damage.duplicated.fetch_add(1, Ordering::Relaxed);
            events
                .entry(now + Duration::from_millis(delay + rng.gen_range(1..=4000)))
                .or_default()
                .push(Datagram {
                    upstream,
                    target: destination,
                    wire: wire.clone(),
                });
        }
        events
            .entry(now + Duration::from_millis(delay))
            .or_default()
            .push(Datagram {
                upstream,
                target: destination,
                wire,
            });
        let queued = events.values().map(Vec::len).sum::<usize>();
        damage
            .peak_events
            .fetch_max(queued as u64, Ordering::Relaxed);
        assert!(queued < 20_000, "relay backlog exceeded its bound");
    }
}

fn payload(side: u8, id: u64) -> Vec<u8> {
    let length = if id % 34 == 0 { 4096 } else { 800 };
    let mut p = vec![side.wrapping_add(id as u8); length];
    p[0] = 0xfe;
    p[1] = side;
    p[2..10].copy_from_slice(&id.to_le_bytes());
    p
}

async fn exchange(
    socket: Arc<RaknetSocket>,
    side: u8,
    duration: Duration,
    total_tx: oneshot::Sender<u64>,
    mut peer_total: oneshot::Receiver<u64>,
) -> u64 {
    let send_socket = socket.clone();
    let sender = tokio::spawn(async move {
        let start = Instant::now();
        let mut count = 0;
        while start.elapsed() < duration {
            let mode = if count % 2 == 0 {
                Reliability::ReliableOrdered
            } else {
                Reliability::Reliable
            };
            send_socket
                .send_with_order_channel(
                    &payload(side, count),
                    mode,
                    if count % 2 == 0 { 3 } else { 7 },
                )
                .await
                .unwrap();
            count += 1;
            sleep(Duration::from_millis(15)).await;
        }
        total_tx.send(count).unwrap();
        send_socket.flush().await.unwrap();
        count
    });
    let mut expected = None;
    let mut received = HashSet::new();
    let mut next_ordered = 0;
    loop {
        if expected.is_some_and(|total| received.len() as u64 == total) {
            break;
        }
        tokio::select! {
            total=&mut peer_total, if expected.is_none()=>{expected=Some(total.unwrap());}
            message=socket.recv()=>{
                let message=message.unwrap();assert!(message.len()>=10);
                let id=u64::from_le_bytes(message[2..10].try_into().unwrap());
                assert_eq!(message,payload(1-side,id),"corrupt payload at id={id}");
                assert!(received.insert(id),"duplicate application delivery at id={id}");
                if id%2==0 {assert_eq!(id,next_ordered,"ordered gap at id={id}");next_ordered+=2;}
            }
        }
    }
    let total = expected.unwrap();
    for id in 0..total {
        assert!(received.contains(&id), "missing application id={id}");
    }
    assert!(
        total >= 256,
        "insufficient traffic to qualify as a soak test"
    );
    assert!(
        timeout(Duration::from_secs(8), socket.recv())
            .await
            .is_err(),
        "late duplicate application delivery"
    );
    sender.await.unwrap();
    total
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit real-time soak: 10 minutes of private-network UDP damage"]
async fn real_udp_soak_delivers_reliable_and_ordered_messages_exactly_once() {
    let seconds = std::env::var("RAKNET_SOAK_SECONDS")
        .map(|v| v.parse::<u64>().unwrap())
        .unwrap_or(600);
    assert!(seconds >= 30, "use >=30 for smoke and 600 for formal soak");
    let parent = std::env::var("TASK_HOST_NETNS").expect("private-network parent marker required");
    assert_ne!(
        std::fs::read_link("/proc/self/ns/net")
            .unwrap()
            .to_string_lossy(),
        parent,
        "host-network fallback forbidden"
    );
    timeout(Duration::from_secs(seconds + 240), async {
        for version in [10, 11] {
            soak_version(version, Duration::from_secs(seconds / 2)).await;
        }
    })
    .await
    .expect("soak delivery/drain deadline exceeded");
}

async fn soak_version(version: u8, duration: Duration) {
    let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    listener.listen().await;
    let front = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy = front.local_addr().unwrap();
    let back = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let damage = Arc::new(Damage::default());
    let pump = tokio::spawn(relay(
        front,
        back,
        listener.local_addr().unwrap(),
        damage.clone(),
        0x5eed + u64::from(version),
    ));
    let client = Arc::new(
        RaknetSocket::connect_with_version(&proxy, version)
            .await
            .unwrap(),
    );
    let server = Arc::new(listener.accept().await.unwrap());
    #[cfg(any(feature = "send-policy", feature = "recovery-policy"))]
    for socket in [&client, &server] {
        #[cfg(feature = "send-policy")]
        socket
            .set_send_options(rust_raknet::SendOptions::default())
            .await
            .unwrap();
        #[cfg(feature = "recovery-policy")]
        socket
            .set_recovery_options(rust_raknet::RecoveryOptions {
                deadline_driven: true,
                tail_probe_min_delay: Some(Duration::from_millis(10)),
                reset_backoff_on_progress: true,
            })
            .await
            .unwrap();
    }
    damage.enabled.store(true, Ordering::Release);
    let start = Instant::now();
    let (client_tx, client_rx) = oneshot::channel();
    let (server_tx, server_rx) = oneshot::channel();
    let (client_received, server_received) = tokio::join!(
        exchange(client.clone(), 0, duration, client_tx, server_rx),
        exchange(server.clone(), 1, duration, server_tx, client_rx)
    );
    eprintln!(
        "UDP-soak complete: version={version}, seconds={:.3}, client_received={client_received}, server_received={server_received}, seen={}, dropped={}, duplicates={}, delayed_acks={}, peak_events={}",
        start.elapsed().as_secs_f64(),
        damage.seen.load(Ordering::Relaxed),
        damage.dropped.load(Ordering::Relaxed),
        damage.duplicated.load(Ordering::Relaxed),
        damage.delayed_acks.load(Ordering::Relaxed),
        damage.peak_events.load(Ordering::Relaxed)
    );
    assert!(damage.dropped.load(Ordering::Relaxed) > 0);
    assert!(damage.duplicated.load(Ordering::Relaxed) > 0);
    assert!(damage.delayed_acks.load(Ordering::Relaxed) > 0);
    damage.enabled.store(false, Ordering::Release);
    client.close().await.unwrap();
    server.close().await.unwrap();
    listener.close().await.unwrap();
    pump.abort();
    let _ = pump.await;
}
