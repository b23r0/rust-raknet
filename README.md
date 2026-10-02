<div align="center">

# rust-raknet

**Async RakNet transport for Rust, with Minecraft Bedrock proxy support.**

[![Build](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet)
[![Documentation](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2)](https://discord.gg/ZKtYMvDFN4)

[Get started](#get-started) · [Bedrock proxies](#minecraft-bedrock) · [Benchmarks](#benchmarks) · [Contributing](#contributing)

</div>

`rust-raknet` implements RakNet handshakes, reliability, ordering and fragmentation
on top of Tokio. Use it to exchange messages over UDP or forward Bedrock traffic.
The crate also has a separate TCP forwarder for NetherNet signaling.

- Client and listener APIs, with all five RakNet reliability modes.
- Bounded send queues, backpressure and selective retransmission.
- Configurable MTU, accept backlog and UDP receive buffers.
- Optional Linux receive socket sharding for busy servers.
- Pure Rust, MIT licensed. Linux, Windows, macOS and BSD.

**Requirements:** Rust 1.85+ and Tokio 1.38+.

## Get started

```toml
[dependencies]
rust-raknet = "0.16.0"
tokio = { version = "1.38", features = ["full"] }
```

### Echo server

```rust
use rust_raknet::{RaknetListener, Reliability, error::Result};

#[tokio::main]
async fn main() -> Result<()> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let mut listener = RaknetListener::bind(&address).await?;
    listener.listen().await;

    loop {
        let socket = listener.accept().await?;
        tokio::spawn(async move {
            while let Ok(message) = socket.recv().await {
                if socket.send(&message, Reliability::ReliableOrdered).await.is_err() {
                    break;
                }
            }
        });
    }
}
```

### Client

```rust
use rust_raknet::{RaknetSocket, Reliability, error::Result};

#[tokio::main]
async fn main() -> Result<()> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let socket = RaknetSocket::connect(&address).await?;
    socket.send(&[0xfe, 1, 2, 3], Reliability::ReliableOrdered).await?;
    let reply = socket.recv().await?;
    println!("Echo: {reply:?}");
    socket.close().await
}
```

Application messages must be nonempty and start with `0xfe`. Keep receiving while
sending sustained bursts so the other end can make progress.

| Reliability | Delivery | Ordering |
| --- | --- | --- |
| `Unreliable` | Best effort | None |
| `UnreliableSequenced` | Best effort | Older messages are discarded |
| `Reliable` | Retransmitted until acknowledged | None |
| `ReliableOrdered` | Retransmitted until acknowledged | In order, per channel |
| `ReliableSequenced` | Reliable, with sequencing | Older messages are discarded |

See the [API docs](https://docs.rs/rust-raknet/latest/rust_raknet/) for
`send_with_order_channel`, `flush`, discovery and listener options.

## Minecraft Bedrock

The crate transports Bedrock packets as bytes. It does not implement Xbox login
or decode the game protocol. Choose the proxy for the server's transport setting:

| Server setting | Example | Traffic forwarded |
| --- | --- | --- |
| `transport=raknet` | [`example/proxy`](example/proxy) | RakNet/UDP game traffic |
| `transport=nethernet` | [`example/nethernet_proxy`](example/nethernet_proxy) | TCP signaling only |

### RakNet proxy

```sh
cargo run --release --manifest-path example/proxy/Cargo.toml -- \
  -l 127.0.0.1:19144 -r 127.0.0.1:19142
```

The proxy keeps the client's RakNet version when connecting upstream. Both
forwarding directions run concurrently; upstream handshakes time out after 10 s.
On Linux, add `--socket-shards 4` to opt into four receive sockets. The default is
one; several frontend shards can be slower when they feed a single upstream socket.

### NetherNet signaling

```rust
use rust_raknet::NetherNetProxy;

async fn forward_signaling() -> std::io::Result<()> {
    let proxy = NetherNetProxy::bind(
        "127.0.0.1:19144".parse().unwrap(),
        "127.0.0.1:19142".parse().unwrap(),
    ).await?;
    proxy.run().await
}
```

After signaling, gameplay travels directly between client and server over WebRTC.
It does **not** pass through `NetherNetProxy`. The client must be able to reach
the server's advertised ICE candidate. See Mojang's
[NetherNet guide](https://github.com/Mojang/bedrock-protocol-docs/blob/main/additional_docs/NetherNetOnboardingGuide.md)
for the connection flow.

```sh
cargo run --manifest-path example/nethernet_proxy/Cargo.toml -- \
  --listen 127.0.0.1:19144 --upstream 127.0.0.1:19142
```

### Server discovery

```sh
cargo run --manifest-path example/bedrock_ping/Cargo.toml -- play.example.com:19132
```

This sends an unconnected RakNet ping and prints the server name, game version,
player counts and MOTD.

## Configuration

<details>
<summary><b>MTU negotiation</b></summary>

The default nominal MTU is 1,400 B. Both peers can opt into another limit:

```rust
use rust_raknet::{RaknetListener, RaknetSocket, error::Result};

async fn configured_connection() -> Result<()> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let mut listener = RaknetListener::bind_with_maximum_mtu(&address, 1428).await?;
    listener.listen().await;
    let client = RaknetSocket::connect_with_version_and_mtu(&address, 11, 1428).await?;
    assert_eq!(client.mtu(), 1428);
    client.close().await?;
    listener.close().await
}
```

Limits must be 61–1,492 B. Negotiation takes the smaller peer limit; `mtu()` returns
it. The nominal MTU reserves 28 B for IPv4/UDP headers. Account for IPv6 or tunnel
overhead when choosing a size. This is not path MTU discovery.

</details>

<details>
<summary><b>Linux receive socket sharding</b></summary>

```rust
use std::num::NonZeroUsize;
use rust_raknet::{RaknetListener, error::Result};

async fn listen() -> Result<RaknetListener> {
    let address = "127.0.0.1:19132".parse().unwrap();
    let mut listener = RaknetListener::bind_with_socket_shards(
        &address,
        NonZeroUsize::new(4).unwrap(),
    ).await?;
    listener.listen().await;
    Ok(listener)
}
```

`SO_REUSEPORT` distributes peer flows across the sockets. They share one GUID,
MOTD and accept backlog, and close as one listener. The shard count stays fixed
until shutdown. The runtime needs enough worker threads to use them; benchmark
your workload before changing the default of one socket.

</details>

<details>
<summary><b>Queues, receive limits and shutdown</b></summary>

- Sends wait for the connected handshake and for space in a full send queue.
  Reliable delivery uses a 64-datagram flight window. A normal burst queues up to
  256 KiB including frame overhead. A larger `ReliableOrdered` message can enter
  an empty queue, subject to a 64 MiB budget and 65,536 fragments. The budget counts
  payload plus 128 B per frame; it is not a process memory limit.
- Receive reordering allows 65,536 reliable indexes and 64 MiB of ordered payload.
  Fragment reassembly allows 1,024 groups, 65,536 fragments per group and 64 MiB
  including frame overhead. A group stalled for 60 s closes the connection.
  Invalid or excessive receive state closes the connection instead of silently
  dropping acknowledged data.
- `bind()` requests a 2 MiB UDP receive buffer; the OS may clamp it.
  `bind_with_receive_buffer_size()` requests a different size. `from_std()` keeps
  the supplied socket's buffer settings.
- The default accept backlog is 128. Set `with_accept_backlog(NonZeroUsize)` before
  `listen()` to change it. A full backlog defers new offline handshakes until
  their next retry. It does not cap active sessions.
- `NetherNetProxy` accepts up to 1,024 active signaling connections by default.
  Use `with_connection_limit(NonZeroUsize)` to change it. Upstream connects time
  out after 10 s. Cancelling `run()` cancels its forwarding tasks too.

</details>

## Benchmarks

These measurements compare **this `rust-raknet` implementation** with a TCP echo
program and the official [C KCP implementation](https://github.com/skywind3000/kcp).
They are not measurements of the RakNet protocol in general.

Measured **October 1–2, 2026** on an Intel Core i7-9700F (8 logical CPUs), Linux
x86_64, Rust 1.98.1, Tokio 1.53.1 and GCC 13.3.0, using release builds. Runs used
an isolated loopback network with MTU 1,500 B, GSO/GRO limited to one packet,
`tc netem` and `nice 10`. Host network settings were unchanged.

Throughput counts echoed application payload **per direction**, excluding headers
and ACKs. **Higher is better.** Tables show the median of three runs; expand the
ranges to see the variation. 1 MiB = 1,048,576 B.

### Single connection

Rust drivers use four workers; C KCP uses one event loop per process. The
configured MTUs give `rust-raknet` and C KCP equal IPv4 packet budgets; details
are below. These numbers do not compare CPU efficiency.

| Network profile | Payload / burst count | TCP | **rust-raknet** | C KCP |
| --- | ---: | ---: | ---: | ---: |
| No injected loss | 800 B / 200,000 messages | 150.97 MiB/s | 188.42 MiB/s | 153.20 MiB/s |
| 1% loss | 800 B / 200,000 messages | 20.08 MiB/s | 174.84 MiB/s | 143.60 MiB/s |
| 5% loss | 800 B / 200,000 messages | 1.07 MiB/s | 168.94 MiB/s | 120.06 MiB/s |
| No injected loss, small packets | 64 B / 300,000 messages | 13.75 MiB/s | 16.60 MiB/s | 12.38 MiB/s |
| No injected loss, fragmented messages | 4,096 B / 50,000 messages | 344.51 MiB/s | 316.13 MiB/s | 254.99 MiB/s |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.12 MiB/s | 2.94 MiB/s | 2.95 MiB/s |

<details>
<summary>Throughput ranges and setup</summary>

| Network profile | Payload / burst count | TCP throughput (median, range) | rust-raknet throughput (median, range) | C KCP throughput (median, range) |
| --- | ---: | ---: | ---: | ---: |
| No injected loss | 800 B / 200,000 messages | 150.97 MiB/s (150.44–153.57 MiB/s) | 188.42 MiB/s (184.14–189.43 MiB/s) | 153.20 MiB/s (151.82–154.32 MiB/s) |
| 1% loss | 800 B / 200,000 messages | 20.08 MiB/s (18.88–43.51 MiB/s) | 174.84 MiB/s (171.90–186.40 MiB/s) | 143.60 MiB/s (143.42–147.77 MiB/s) |
| 5% loss | 800 B / 200,000 messages | 1.07 MiB/s (1.02–1.08 MiB/s) | 168.94 MiB/s (150.75–170.40 MiB/s) | 120.06 MiB/s (118.66–122.54 MiB/s) |
| No injected loss, small packets | 64 B / 300,000 messages | 13.75 MiB/s (13.49–13.81 MiB/s) | 16.60 MiB/s (15.94–16.79 MiB/s) | 12.38 MiB/s (12.18–12.42 MiB/s) |
| No injected loss, fragmented messages | 4,096 B / 50,000 messages | 344.51 MiB/s (343.29–350.93 MiB/s) | 316.13 MiB/s (313.54–317.18 MiB/s) | 254.99 MiB/s (250.86–259.83 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.12 MiB/s (1.07–1.19 MiB/s) | 2.94 MiB/s (2.71–3.10 MiB/s) | 2.95 MiB/s (2.86–2.97 MiB/s) |

Each run has 100 warmups, 300 sequential RTT samples and the measured burst.
All drivers keep at most 64 messages awaiting verified echoes and refill on each
echo. Run order alternates. `rust-raknet` uses `ReliableOrdered`; TCP uses
`TCP_NODELAY`, length-prefixed records, whole-record writes and a reused buffer.
The official C KCP core is unchanged, pinned to
[`b1a7a21`](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766).

The Rust processes use four Tokio workers; C KCP uses one event loop per process.
Throughput runs have no CPU affinity. This is not an equal-CPU-cost comparison.

`rust-raknet` uses a nominal MTU of 1,428 B here, including 28 B of IPv4/UDP
headers. C KCP uses a 1,400 B UDP MTU, excluding those headers. Their IPv4 packet
budgets match, and both split a 4,096 B message into three fragments. The default
`rust-raknet` MTU is still 1,400 B, which needs four fragments for that message.
C KCP uses message mode, send/receive windows of 64/128 segments,
`nodelay(1, 10, 2, 1)`, immediate writes/ACKs, and no FEC or encryption.
`rust-raknet` keeps its normal flight window and retry timings.

All three implementations completed 18/18 runs each. `rust-raknet` leads C KCP in
five profiles; with delay and loss, they are effectively tied. TCP leads the
4 KiB profile.

</details>

### Single-connection latency

RTT is round-trip latency. **Lower is better.** Values are the medians of
run-level p50 / p95 / p99 percentiles from three runs, each with 300 warmups and
3,000 sequential samples.

| Network profile | TCP RTT (p50 / p95 / p99) | rust-raknet RTT (p50 / p95 / p99) | C KCP RTT (p50 / p95 / p99) |
| --- | ---: | ---: | ---: |
| No injected loss, CPU affinity | 16.1 µs / 75.2 µs / 87.1 µs | 17.6 µs / 22.4 µs / 48.0 µs | 13.8 µs / 57.5 µs / 62.0 µs |

Client and server are pinned to separate CPUs. Each Rust process has four workers
on its assigned CPU; C KCP has one event loop. These samples precede the burst and
do not measure delivery latency under sustained load.

### Concurrent connections

800 B messages, a sliding window of 16 per connection, and **1,048,576 verified
ordered echoes** per run. All three implementations use four workers and four
server sockets with `SO_REUSEPORT`, with separate sets of four client/server CPUs.
These are transport sessions, not authenticated Minecraft players.

| Connections | Injected loss | TCP | **rust-raknet** | C KCP |
| ---: | ---: | ---: | ---: | ---: |
| 64 connections | 0% | 713.62 MiB/s | 543.43 MiB/s | 362.82 MiB/s |
| 256 connections | 0% | 598.00 MiB/s | 556.90 MiB/s | 415.87 MiB/s |
| 1,024 connections | 0% | 591.75 MiB/s | 497.71 MiB/s | 380.07 MiB/s |
| 2,048 connections | 0% | 586.90 MiB/s | 440.64 MiB/s | 361.93 MiB/s |
| 64 connections | 1% | 390.96 MiB/s | 520.11 MiB/s | 377.12 MiB/s |
| 256 connections | 1% | 519.37 MiB/s | 511.55 MiB/s | 384.26 MiB/s |
| 1,024 connections | 1% | 424.33 MiB/s | 455.86 MiB/s | 390.54 MiB/s |
| 2,048 connections | 1% | 423.78 MiB/s | 389.14 MiB/s | 335.09 MiB/s |

TCP leads all four clean groups. At 1% loss, `rust-raknet` leads TCP at 64 and
1,024 connections; TCP leads at 256 and 2,048. `rust-raknet` leads C KCP in all
eight groups. Small differences with overlapping ranges need more runs before
calling a winner.

<details>
<summary>Concurrent throughput ranges and setup</summary>

| Connections | Injected loss | TCP throughput (median, range) | rust-raknet throughput (median, range) | C KCP throughput (median, range) | rust-raknet / C KCP median |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 64 connections | 0% | 713.62 MiB/s (700.90–714.45 MiB/s) | 543.43 MiB/s (536.83–572.64 MiB/s) | 362.82 MiB/s (361.95–377.37 MiB/s) | 149.8% |
| 256 connections | 0% | 598.00 MiB/s (581.63–620.48 MiB/s) | 556.90 MiB/s (526.97–562.89 MiB/s) | 415.87 MiB/s (329.26–424.58 MiB/s) | 133.9% |
| 1,024 connections | 0% | 591.75 MiB/s (562.39–594.38 MiB/s) | 497.71 MiB/s (466.01–523.55 MiB/s) | 380.07 MiB/s (378.09–415.04 MiB/s) | 131.0% |
| 2,048 connections | 0% | 586.90 MiB/s (579.19–593.85 MiB/s) | 440.64 MiB/s (429.02–452.66 MiB/s) | 361.93 MiB/s (310.89–376.98 MiB/s) | 121.7% |
| 64 connections | 1% | 390.96 MiB/s (388.32–424.96 MiB/s) | 520.11 MiB/s (491.58–583.39 MiB/s) | 377.12 MiB/s (358.86–406.28 MiB/s) | 137.9% |
| 256 connections | 1% | 519.37 MiB/s (440.41–549.12 MiB/s) | 511.55 MiB/s (462.67–513.39 MiB/s) | 384.26 MiB/s (372.07–406.03 MiB/s) | 133.1% |
| 1,024 connections | 1% | 424.33 MiB/s (416.75–478.65 MiB/s) | 455.86 MiB/s (424.03–468.14 MiB/s) | 390.54 MiB/s (372.44–415.87 MiB/s) | 116.7% |
| 2,048 connections | 1% | 423.78 MiB/s (114.57–465.28 MiB/s) | 389.14 MiB/s (360.29–395.85 MiB/s) | 335.09 MiB/s (323.29–356.21 MiB/s) | 116.1% |

Measured October 2, 2026. Order rotates across TCP / `rust-raknet` / C KCP. Each
connection completes 20 sequential RTT samples before a shared start barrier.
Setup and those samples are outside the throughput timer; connections stay open
until every burst finishes. All three implementations completed 24/24 runs each
(72/72 total).

TCP uses `TCP_NODELAY`, complete-record writes and reused buffers. Its four
listeners distribute accepts; accepted streams each have their own socket. UDP
sockets distribute datagrams. `rust-raknet` uses the default nominal MTU of
1,400 B; C KCP uses a 1,400 B UDP MTU. Their IPv4 budgets differ by 28 B, but an
800 B message fits one datagram in both. TCP can combine several records in a
segment, so packet loss percentages do not imply equal lost-message counts.

The 2,048-connection TCP loss case spans 114.57–465.28 MiB/s. Random loss affects
data and ACKs in both directions, and each run has a different trace. UDP buffers
can also overflow under saturation, even at 0% injected loss. Check UDP error
counters alongside netem counters when reproducing these results. CPU affinity
does not reserve the CPUs exclusively.

</details>

Build instructions and workload details are in
[the benchmark README](example/test_benchmark/README.md) and
[the C KCP adapters](example/test_benchmark/kcp/README.md).

## Contributing

Bug reports should include the crate version, a small reproducer and what you
expected to happen. For larger changes, open an issue before writing the patch.

Work in a disposable container, VM or task copy with private caches. Keep network
tests in a private network namespace. Before opening a pull request:

```sh
cargo fmt --all -- --check
cargo build --all-targets
cargo test --all-targets
```

Include the checks you ran. For transport or performance changes, include the
workload and enough detail to reproduce it.

To bump the crate version, run `python3 scripts/set-version.py NEW_VERSION` in the task
copy, using the next release version (for example, `0.16.0`). This updates `Cargo.toml`, the
Get Started dependency and the crate documentation together. CI checks they match.

### Contributors

Thanks to everyone who has contributed commits:

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


---

Licensed under [MIT](LICENSE). See the
[RakNet protocol reference](http://www.jenkinssoftware.com/raknet/manual/index.html).
This project is not affiliated with Jenkins Software LLC or RakNet.
