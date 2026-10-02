# rust-raknet [![GitHub Actions](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml) [![ChatOnDiscord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/ZKtYMvDFN4) [![Crate](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet) [![Crate](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
RakNet Protocol implementation by Rust.

Raknet is a reliable udp transport protocol that is generally used for communication between game clients and servers, and is used by Minecraft Bedrock Edtion for underlying communication.

Raknet protocol supports various reliability options, and has better transmission performance than TCP in unstable network environments. This project is an incomplete implementation of the protocol by reverse engineering.

Requires *Tokio 1.38 or newer* asynchronous runtime support.

Reference : http://www.jenkinssoftware.com/raknet/manual/index.html

_This project is not affiliated with Jenkins Software LLC nor RakNet._

# Features

* Async
* MIT License
* Pure Rust implementation
* Fast Retransmission
* Selective Retransmission (TCP/Full Retransmission)
* Non-delayed ACK (TCP/Delayed ACK)
* RTO Not Doubled (TCP/RTO Doubled)
* Linux/Windows/Mac/BSD support
* Compatible with Minecraft
* NetherNet TCP signaling forwarding API

# Get Started

```toml
# Cargo.toml
[dependencies]
rust-raknet = "0.15"
```

Documentation : https://docs.rs/rust-raknet/latest/rust_raknet/

# Reliability

- [x] unreliable
- [x] unreliable sequenced
- [x] reliable
- [x] reliable ordered
- [x] reliable sequenced

# Example

```rs
//server

async fn serve(){
    let mut listener = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    listener.listen().await;
    loop{
        let socket = listener.accept().await.unwrap();
        let buf = socket.recv().await.unwrap();
        if buf[0] == 0xfe{
            //do something
        }
    }
    listener.close().await.unwrap();
}

```

```rs
//client

async fn connect(){
    let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    let buf = socket.recv().await.unwrap();
    if buf[0] == 0xfe{
        //do something
    }
    socket.close().await.unwrap();
}
```

## Explicit MTU negotiation

The optional MTU and Linux sharding APIs below are available in the current
repository source.

The ordinary client and listener APIs retain their 1,400-byte nominal MTU.
Applications that know their network path can opt into another limit:

```rust
use rust_raknet::{RaknetListener, RaknetSocket};

async fn configured_connection() -> rust_raknet::error::Result<()> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let mut listener = RaknetListener::bind_with_maximum_mtu(&address, 1428).await?;
    listener.listen().await;
    let client = RaknetSocket::connect_with_version_and_mtu(&address, 11, 1428).await?;
    assert_eq!(client.mtu(), 1428);
    client.close().await?;
    listener.close().await
}
```

Both limits must be 61–1,492 bytes. Negotiation uses the smaller peer limit;
`mtu()` returns the negotiated value. The nominal RakNet MTU reserves 28 bytes
for IPv4/UDP headers. Choose a value the actual path can carry, accounting for
IPv6 or tunnel overhead when applicable. Larger MTUs can reduce fragmentation;
this API does not discover path MTU or change the host interface configuration.

## Linux receive socket sharding

High-concurrency servers can opt into a fixed group of receive sockets:

```rust
use std::num::NonZeroUsize;
use rust_raknet::RaknetListener;

async fn listen() -> rust_raknet::error::Result<RaknetListener> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let mut listener = RaknetListener::bind_with_socket_shards(
        &address,
        NonZeroUsize::new(4).unwrap(),
    ).await?;
    listener.listen().await;
    Ok(listener)
}
```

This Linux API uses `SO_REUSEPORT` to distribute peer flows. The group shares one
server GUID, MOTD and accept backlog, and closes as one listener. Its size is fixed
until shutdown. One shard uses the ordinary `bind` path; the default API continues
to use one socket. The runtime needs enough worker threads to process the shards. Measure the
workload before choosing a shard count: a proxy feeding a single upstream socket
can be slower with several frontend shards. The proxy default remains one.

The RakNet proxy example accepts `--socket-shards 4` on Linux:

```sh
cargo run --release --manifest-path example/proxy/Cargo.toml -- \
    -l 127.0.0.1:19144 -r 127.0.0.1:19142 --socket-shards 4
```

# Bedrock server discovery

The example/bedrock_ping program sends an unconnected RakNet ping and prints the Bedrock server name, game version, player counts, and MOTD:

    cargo run --manifest-path example/bedrock_ping/Cargo.toml -- play.example.com:19132

This example covers server-list status discovery. The crate forwards opaque Bedrock packets; it does not implement Xbox authentication or decode game packets.

## Bedrock transport compatibility

The `example/proxy` program is a RakNet/UDP proxy. It works with Bedrock servers configured as `transport=raknet`; it cannot accept the TCP/WebRTC transport that recent Bedrock Dedicated Server versions use by default. The RakNet proxy negotiates the upstream RakNet version accepted from its client. Both forwarding directions run concurrently so backpressure in one direction does not block receiving in the other. Upstream handshakes time out after 10 seconds.

For servers configured as `transport=nethernet`, the library exposes `NetherNetProxy` to forward the HTTP signaling connection:

```rust
async fn start() -> std::io::Result<()> {
    let proxy = rust_raknet::NetherNetProxy::bind(
        "127.0.0.1:19144".parse().unwrap(),
        "127.0.0.1:19142".parse().unwrap(),
    )
    .await?;
    proxy.run().await
}
```

The runnable example uses this API:

    cargo run --manifest-path example/nethernet_proxy/Cargo.toml -- --listen 127.0.0.1:19144 --upstream 127.0.0.1:19142

This API forwards only NetherNet's TCP signaling connection. After SDP signaling, Bedrock sends game traffic directly to the server over WebRTC; it does not pass through this proxy. The client's network must be able to reach the server's advertised ICE candidate. For the protocol flow and NAT guidance, see Mojang's [NetherNet signaling guide](https://github.com/Mojang/bedrock-protocol-docs/blob/main/additional_docs/NetherNetOnboardingGuide.md).

## Transport limits and shutdown

Application sends wait for the connected handshake and apply asynchronous backpressure when the send queue is full. Reliable delivery keeps a 64-datagram flight window. A burst normally queues up to 256 KiB including frame overhead; a single larger `ReliableOrdered` message can be admitted when the queue is empty, up to a 64 MiB queue budget that charges payload bytes plus 128 bytes per frame. Messages are also limited to 65,536 fragments so a local receiver can reassemble every message the sender admits. This accounting limit is not a process RSS limit.

Receive reordering is limited to 65,536 reliable indexes and 64 MiB of ordered payload. Fragment reassembly allows at most 1,024 concurrent groups, 65,536 fragments per group, and 64 MiB including frame overhead. An incomplete group that makes no progress for 60 seconds closes the connection. Invalid or excessive receive state is disconnected rather than acknowledged and silently discarded. Applications should consume incoming messages concurrently with sustained sends.

`RaknetListener::bind()` requests a 2 MiB receive buffer on its own UDP socket to absorb bursts from many connections. The OS can clamp this request to its existing limits. Use `bind_with_receive_buffer_size()` to request another size, or pass a socket configured by your application to `from_std()`, which preserves its buffer settings.

A `RaknetListener` buffers up to 128 connections awaiting `accept()` by default. Call `with_accept_backlog(NonZeroUsize)` before `listen()` to configure this bound. A full backlog defers new offline handshakes until their next retry, instead of completing negotiation and immediately disconnecting them. The backlog limits pending accepts, not active sessions; keep accepting and consume each connection's data concurrently.

`NetherNetProxy` limits active signaling connections to 1,024 by default; use `with_connection_limit(NonZeroUsize)` to choose another limit. Upstream connection attempts time out after 10 seconds. Cancelling `run()` also cancels its active forwarding tasks.

# Benchmark

Measured on **2026-10-01–02**, comparing this tree with TCP and the
[official C KCP implementation](example/test_benchmark/kcp/README.md). RakNet uses
`ReliableOrdered`; TCP uses length-prefixed records, `TCP_NODELAY`, whole-record
writes and a reused server buffer. Both C and Rust drivers verify complete echoes
and reuse payload templates. The upstream KCP core is unchanged.

Environment: Intel Core i7-9700F (8 logical CPUs), Linux x86_64, Rust 1.98.1,
Tokio 1.53.1, GCC 13.3.0, release builds. Tests ran in a private user/network
namespace: loopback MTU 1,500 B, GSO/GRO limited to one packet, namespace-local
`tc netem`, and process priority `nice 10`. Host network settings were unchanged.
CPU affinity does not reserve a core exclusively.

### Single-connection throughput

**Higher is better.** Values count echoed application payload per direction in
MiB/s (1 MiB = 1,048,576 B), excluding headers and ACKs. Cells show the median and
range of three runs, alternating protocol order. Each run has 100 warmups and
300 sequential RTT samples before the measured burst. Every driver keeps at most
64 messages awaiting verified echoes, refilling available slots without an idle
poll between batches. Throughput runs have no CPU affinity.

The Rust processes use four Tokio workers; the single-connection C adapter uses
one event loop per process. These are throughput comparisons of those configured
applications, not comparisons at equal CPU consumption.

For equal IPv4 packet budgets, this table explicitly configures RakNet's nominal
MTU to **1,428 B**, including 28 B of IPv4/UDP overhead. KCP's UDP MTU is
**1,400 B**, excluding that overhead. The ordinary RakNet APIs retain their
**1,400 B** default. The configured size lets a 4,096 B message use three
fragments in both protocols; the default RakNet size needs four. KCP uses message
mode, send/receive windows of 64/128 segments, `nodelay(1, 10, 2, 1)`, immediate
writes/ACKs, and no FEC or encryption. RakNet keeps its normal 64-datagram flight
window and retry timings.

| Network profile | Payload / burst count | TCP throughput (median, range) | RakNet throughput (median, range) | C KCP throughput (median, range) |
| --- | ---: | ---: | ---: | ---: |
| No injected loss | 800 B / 200,000 messages | 150.97 MiB/s (150.44–153.57 MiB/s) | 188.42 MiB/s (184.14–189.43 MiB/s) | 153.20 MiB/s (151.82–154.32 MiB/s) |
| 1% loss | 800 B / 200,000 messages | 20.08 MiB/s (18.88–43.51 MiB/s) | 174.84 MiB/s (171.90–186.40 MiB/s) | 143.60 MiB/s (143.42–147.77 MiB/s) |
| 5% loss | 800 B / 200,000 messages | 1.07 MiB/s (1.02–1.08 MiB/s) | 168.94 MiB/s (150.75–170.40 MiB/s) | 120.06 MiB/s (118.66–122.54 MiB/s) |
| No injected loss, small packets | 64 B / 300,000 messages | 13.75 MiB/s (13.49–13.81 MiB/s) | 16.60 MiB/s (15.94–16.79 MiB/s) | 12.38 MiB/s (12.18–12.42 MiB/s) |
| No injected loss, fragmented messages | 4,096 B / 50,000 messages | 344.51 MiB/s (343.29–350.93 MiB/s) | 316.13 MiB/s (313.54–317.18 MiB/s) | 254.99 MiB/s (250.86–259.83 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.12 MiB/s (1.07–1.19 MiB/s) | 2.94 MiB/s (2.71–3.10 MiB/s) | 2.95 MiB/s (2.86–2.97 MiB/s) |

All three protocols completed **18/18** throughput runs. In this configuration,
RakNet's median exceeds C KCP in the clean, small, fragmented and 1%/5% loss
profiles. With delay and loss their medians are effectively equal; this does not
establish a performance ordering across other networks or CPU budgets.

### Single-connection latency

**Lower is better.** RTT is round-trip latency. Each cell lists p50 / p95 / p99 as
medians of run-level percentiles. Each protocol has three runs, 3,000 sequential
samples after 300 warmups, with separate client/server CPU affinities. Rust uses
four workers pinned to the same per-process CPU; C uses one event loop.

| Network profile | TCP RTT (p50 / p95 / p99) | RakNet RTT (p50 / p95 / p99) | C KCP RTT (p50 / p95 / p99) |
| --- | ---: | ---: | ---: |
| No injected loss, CPU affinity | 16.1 µs / 75.2 µs / 87.1 µs | 17.6 µs / 22.4 µs / 48.0 µs | 13.8 µs / 57.5 µs / 62.0 µs |

### Concurrent throughput

Measured on **2026-10-02** in a fresh comparison including TCP, RakNet and C KCP.
All three protocols use four workers, four server sockets with `SO_REUSEPORT`,
separate sets of four client/server CPUs, 800 B messages, and a sliding application
window of 16 messages per connection. TCP uses `TCP_NODELAY`, four-byte
length-prefixed records, complete-record writes and reused record buffers. TCP
listeners distribute connection acceptance; each accepted stream has its own
socket. UDP sockets distribute incoming datagrams.

Each run verifies **1,048,576 ordered echoes** in its measured burst. Cells show
the median and range of three runs, rotating TCP / RakNet / C KCP order. Setup and
20 preceding sequential RTTs per connection are excluded from throughput; every
connection stays open until all bursts finish. These are synthetic transport
sessions, not authenticated Minecraft players.

This table uses the ordinary nominal RakNet MTU of **1,400 B** and KCP UDP MTU of
**1,400 B**. Their IPv4 budgets differ by 28 B, but each 800 B message fits one
datagram in both protocols. TCP may combine several records in a segment; packet
loss percentages do not imply the same number of lost application messages.
Do not extrapolate this table to fragmented traffic.

| Connections | Injected loss | TCP throughput (median, range) | RakNet throughput (median, range) | C KCP throughput (median, range) | RakNet / C KCP median |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 64 connections | 0% | 713.62 MiB/s (700.90–714.45 MiB/s) | 543.43 MiB/s (536.83–572.64 MiB/s) | 362.82 MiB/s (361.95–377.37 MiB/s) | 149.8% |
| 256 connections | 0% | 598.00 MiB/s (581.63–620.48 MiB/s) | 556.90 MiB/s (526.97–562.89 MiB/s) | 415.87 MiB/s (329.26–424.58 MiB/s) | 133.9% |
| 1,024 connections | 0% | 591.75 MiB/s (562.39–594.38 MiB/s) | 497.71 MiB/s (466.01–523.55 MiB/s) | 380.07 MiB/s (378.09–415.04 MiB/s) | 131.0% |
| 2,048 connections | 0% | 586.90 MiB/s (579.19–593.85 MiB/s) | 440.64 MiB/s (429.02–452.66 MiB/s) | 361.93 MiB/s (310.89–376.98 MiB/s) | 121.7% |
| 64 connections | 1% | 390.96 MiB/s (388.32–424.96 MiB/s) | 520.11 MiB/s (491.58–583.39 MiB/s) | 377.12 MiB/s (358.86–406.28 MiB/s) | 137.9% |
| 256 connections | 1% | 519.37 MiB/s (440.41–549.12 MiB/s) | 511.55 MiB/s (462.67–513.39 MiB/s) | 384.26 MiB/s (372.07–406.03 MiB/s) | 133.1% |
| 1,024 connections | 1% | 424.33 MiB/s (416.75–478.65 MiB/s) | 455.86 MiB/s (424.03–468.14 MiB/s) | 390.54 MiB/s (372.44–415.87 MiB/s) | 116.7% |
| 2,048 connections | 1% | 423.78 MiB/s (114.57–465.28 MiB/s) | 389.14 MiB/s (360.29–395.85 MiB/s) | 335.09 MiB/s (323.29–356.21 MiB/s) | 116.1% |

All three protocols completed **24/24** concurrent runs (**72/72** total). TCP's
median leads all four clean groups. At 1% loss, RakNet leads TCP at 64 and 1,024
connections; TCP leads at 256 and 2,048. RakNet exceeds C KCP in all eight groups.
At 2,048 connections with loss, TCP spans 114.57–465.28 MiB/s, so its median alone
hides substantial variation. Small median differences and overlapping ranges do
not establish a stable ordering across other networks or CPU budgets.

Random loss affects data and ACKs in both directions, and traces differ between
runs. Even with no injected loss, namespace-local UDP receive buffers can overflow
under saturation. Scheduling, queueing and retransmissions affect the ranges and
tail latency; inspect UDP error counters as well as netem counters when reproducing
results. The sequential RTT phase does not measure latency during the burst.

See the [benchmark instructions](example/test_benchmark/README.md) and
[C adapters](example/test_benchmark/kcp/README.md) for isolated reproduction.

## Contributing

Contributions are welcome! You can help by reporting bugs, suggesting features, improving documentation, adding examples, or submitting code changes.

### Send a contribution

1. For a larger change, open an issue first so we can agree on the approach. Bug reports are most helpful with steps to reproduce and expected behavior.
2. Fork the repository and create a focused branch for your change.
3. Work in a disposable container, VM, or task copy with private caches. Keep network tests in a private network namespace. Format the code and run the same build and test checks used by CI:

   ```sh
   cargo fmt --all -- --check
   cargo build --all-targets
   cargo test --all-targets
   ```

4. Open a pull request with a short summary and the checks you ran. For protocol or performance changes, include a reproducer or benchmark details when possible.

### Contributors

A big thank you to everyone who has contributed commits to rust-raknet.

<div align="center">
  <table>
    <tr>
      <td align="center" width="120"><a href="https://github.com/b23r0"><img src="https://github.com/b23r0.png?size=96" width="80" height="80" alt="b23r0's GitHub avatar" /><br /><sub><b>b23r0</b></sub></a></td>
      <td align="center" width="120"><a href="https://github.com/nounfve"><img src="https://github.com/nounfve.png?size=96" width="80" height="80" alt="nounfve's GitHub avatar" /><br /><sub><b>nounfve</b></sub></a></td>
      <td align="center" width="120"><a href="https://github.com/mikhaillav"><img src="https://github.com/mikhaillav.png?size=96" width="80" height="80" alt="mikhaillav's GitHub avatar" /><br /><sub><b>mikhaillav</b></sub></a></td>
      <td align="center" width="120"><a href="https://github.com/AndreasHGK"><img src="https://github.com/AndreasHGK.png?size=96" width="80" height="80" alt="AndreasHGK's GitHub avatar" /><br /><sub><b>AndreasHGK</b></sub></a></td>
      <td align="center" width="120"><a href="https://github.com/minerj101"><img src="https://github.com/minerj101.png?size=96" width="80" height="80" alt="minerj101's GitHub avatar" /><br /><sub><b>minerj101</b></sub></a></td>
    </tr>
  </table>
</div>
