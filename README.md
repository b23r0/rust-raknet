<div align="center">

# rust-raknet

**A high-performance implementation of the RakNet protocol in Rust.**

[![Build](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet)
[![Documentation](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2)](https://discord.gg/ZKtYMvDFN4)

[Get started](#get-started) · [Bedrock proxy](#minecraft-bedrock) · [Benchmarks](#benchmarks) · [Contributing](#contributing)

</div>

`rust-raknet` provides reliable message delivery over UDP for Rust applications.
Built on Tokio, it implements RakNet handshakes, acknowledgements, retransmission,
ordering and fragmentation. Choose the delivery guarantees your application needs.

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
or decode the game protocol. The [proxy example](example/proxy) forwards RakNet UDP
game traffic and requires a backend server configured with `transport=raknet`.

### Enabling RakNet in Bedrock 26.52

As of October 2, 2026, the latest stable Bedrock release is
[26.52](https://feedback.minecraft.net/hc/en-us/articles/49175370527501-Minecraft-Bedrock-Edition-26-52-Hotfix-Changelog).
The tested setup used a Windows 1.26.52 client and Bedrock Dedicated Server
1.26.52.3, with game protocol 2193 and RakNet protocol 11. Joining, movement,
placing and breaking blocks, and the proxy's server-list MOTD all worked through
`rust-raknet` 0.16.0 over RakNet UDP.

Dedicated servers default to NetherNet starting with
[26.50](https://feedback.minecraft.net/hc/en-us/articles/48826825649933-Minecraft-Bedrock-Edition-26-50-Changelog-Wilderness-Bound).
For the tested 26.52 setup, stop the server and edit its `server.properties`:

```properties
transport=raknet
server-port=19142
server-portv6=19143
enable-lan-visibility=false
```

Restart the dedicated server after saving. Setting `transport=raknet` selects the
legacy UDP transport. Disabling LAN visibility avoids the extra default-port
listeners when using a custom port. Keep the backend and proxy in the same
isolated test network; the proxy command below forwards to backend port 19142.

Configure a matching Bedrock advertisement on the proxy listener before calling
`listen()`. In `example/proxy/src/main.rs`, insert this before
`listener.listen().await`:

```rust
listener.set_motd(
    "Rust RakNet Bedrock proxy",
    10,
    "2193",
    "1.26.52",
    "Survival",
    local_address.port(),
).await?;
```

Use the backend's protocol, game version and game mode when adapting this setup.
The listener's default advertisement describes an older Bedrock version. An
incorrect advertisement can prevent a recent client from joining, even when the
UDP handshake works.

### RakNet deprecation in Bedrock

Mojang is moving Bedrock networking to NetherNet. The
[26.60.22/23 Preview changelog](https://feedback.minecraft.net/hc/en-us/articles/48740748263565-Minecraft-Beta-Preview-26-60-22-23)
marks RakNet deprecated and changes the dedicated-server warning about using
another transport into an error. Our 26.52 server also printed a NetherNet-only
error, while the RakNet connection and gameplay tests still succeeded.

The compatibility result above covers the tested 26.52 builds. It does not
establish support for 26.60 or later, and the cited preview note does not give a
confirmed final removal date. Check the release notes and retest before upgrading
a deployment that depends on RakNet. `rust-raknet` implements RakNet UDP;
NetherNet uses WebRTC and requires a different transport implementation.

### RakNet proxy

```sh
cargo run --release --manifest-path example/proxy/Cargo.toml -- \
  -l 127.0.0.1:19144 -r 127.0.0.1:19142
```

For a local test, add `127.0.0.1:19144` to the client's server list. On another
machine, use the proxy's reachable address and frontend port. The client must
join the proxy endpoint for its game traffic to pass through `rust-raknet`.

The proxy keeps the client's RakNet version when connecting upstream. Both
forwarding directions run concurrently; upstream handshakes time out after 10 s.
On Linux, add `--socket-shards 4` to opt into four receive sockets. The default is
one; several frontend shards can be slower when they feed a single upstream socket.

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

</details>

## Benchmarks

These measurements compare **this `rust-raknet` implementation** with a TCP echo
program, the official [C KCP implementation](https://github.com/skywind3000/kcp),
and [quic-go](https://github.com/quic-go/quic-go).
They are not measurements of the RakNet protocol in general.

**Benchmark reference date: October 1, 2026.**

Measured on an Intel Core i7-9700F (8 logical CPUs), Linux x86_64, Rust 1.98.1,
Tokio 1.53.1, GCC 13.3.0 and Go 1.27.1, using optimized builds. Runs used an isolated loopback network with MTU 1,500 B, GSO/GRO limited to one packet,
`tc netem` and `nice 10`. Host network settings were unchanged.

Throughput counts echoed application payload **per direction**, excluding headers
and ACKs. **Higher is better.** Tables show the median of three runs; expand the
ranges to see the variation. 1 MiB = 1,048,576 B.

### Implementations and versions

| Column | GitHub source | Tested version / revision |
| --- | --- | --- |
| TCP | [TCP echo driver](https://github.com/b23r0/rust-raknet/tree/main/example/test_benchmark), [Tokio](https://github.com/tokio-rs/tokio) | Driver 0.1.0; Tokio 1.53.1; Linux TCP stack 7.0.11-76070011-generic |
| rust-raknet | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 0.16.0, transport revision `7f80c57` |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | Pinned commit `b1a7a21`; no release tag used |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

TCP is a protocol, so its column identifies the benchmark driver, runtime and
kernel rather than assigning TCP a project version. The TCP/KCP measurements
predate the 0.16.0 version bump; the measured RakNet transport code is unchanged
in revision `7f80c57`.

The quic-go column was measured in a separate batch using the workloads
specified in each table: a 64-message window for single connections and a
16-message window per concurrent connection. Loss traces are independent across
runs.

QUIC uses one bidirectional reliable, ordered stream per connection. Records have
four-byte little-endian lengths and reuse their buffers. Encryption, stream flow
control and congestion control are enabled. `rust-raknet` and C KCP do not provide
those same features. Their costs are included in the QUIC numbers, so throughput
alone does not capture the difference in guarantees.

### Single connection

Rust and Go drivers use four workers; C KCP uses one event loop per process.
All UDP implementations have equal IPv4 packet budgets here; details are below.
These numbers do not compare CPU efficiency.

| Network profile | Payload / burst count | TCP | **rust-raknet** | C KCP | quic-go |
| --- | ---: | ---: | ---: | ---: | ---: |
| No injected loss | 800 B / 200,000 messages | 150.97 MiB/s | 188.42 MiB/s | 153.20 MiB/s | 114.47 MiB/s |
| 1% loss | 800 B / 200,000 messages | 20.08 MiB/s | 174.84 MiB/s | 143.60 MiB/s | 46.00 MiB/s |
| 5% loss | 800 B / 200,000 messages | 1.07 MiB/s | 168.94 MiB/s | 120.06 MiB/s | 9.51 MiB/s |
| No injected loss, small messages | 64 B / 300,000 messages | 13.75 MiB/s | 16.60 MiB/s | 12.38 MiB/s | 30.81 MiB/s |
| No injected loss, fragmented messages | 4,096 B / 50,000 messages | 344.51 MiB/s | 316.13 MiB/s | 254.99 MiB/s | 173.02 MiB/s |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.12 MiB/s | 2.94 MiB/s | 2.95 MiB/s | 1.57 MiB/s |

<details>
<summary>Throughput ranges and setup</summary>

| Network profile | Payload / burst count | TCP throughput (median, range) | rust-raknet throughput (median, range) | C KCP throughput (median, range) | quic-go throughput (median, range) |
| --- | ---: | ---: | ---: | ---: | ---: |
| No injected loss | 800 B / 200,000 messages | 150.97 MiB/s (150.44–153.57 MiB/s) | 188.42 MiB/s (184.14–189.43 MiB/s) | 153.20 MiB/s (151.82–154.32 MiB/s) | 114.47 MiB/s (108.74–114.96 MiB/s) |
| 1% loss | 800 B / 200,000 messages | 20.08 MiB/s (18.88–43.51 MiB/s) | 174.84 MiB/s (171.90–186.40 MiB/s) | 143.60 MiB/s (143.42–147.77 MiB/s) | 46.00 MiB/s (43.25–46.66 MiB/s) |
| 5% loss | 800 B / 200,000 messages | 1.07 MiB/s (1.02–1.08 MiB/s) | 168.94 MiB/s (150.75–170.40 MiB/s) | 120.06 MiB/s (118.66–122.54 MiB/s) | 9.51 MiB/s (8.56–10.09 MiB/s) |
| No injected loss, small messages | 64 B / 300,000 messages | 13.75 MiB/s (13.49–13.81 MiB/s) | 16.60 MiB/s (15.94–16.79 MiB/s) | 12.38 MiB/s (12.18–12.42 MiB/s) | 30.81 MiB/s (28.72–31.12 MiB/s) |
| No injected loss, fragmented messages | 4,096 B / 50,000 messages | 344.51 MiB/s (343.29–350.93 MiB/s) | 316.13 MiB/s (313.54–317.18 MiB/s) | 254.99 MiB/s (250.86–259.83 MiB/s) | 173.02 MiB/s (120.29–229.28 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.12 MiB/s (1.07–1.19 MiB/s) | 2.94 MiB/s (2.71–3.10 MiB/s) | 2.95 MiB/s (2.86–2.97 MiB/s) | 1.57 MiB/s (1.54–2.05 MiB/s) |

Each run has 100 warmups, 300 sequential RTT samples and the measured burst.
All drivers keep at most 64 messages awaiting verified echoes and refill on each
echo. TCP / `rust-raknet` / C KCP run order alternates; quic-go was measured in a
separate batch with the same workload. `rust-raknet` uses `ReliableOrdered`; TCP uses
`TCP_NODELAY`, length-prefixed records, whole-record writes and a reused buffer.
The official C KCP core is unchanged, pinned to
[`b1a7a21`](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766).

The Rust processes use four Tokio workers and quic-go uses `GOMAXPROCS=4`;
C KCP uses one event loop per process. All servers use one listening socket.
Throughput runs have no CPU affinity. This is not an equal-CPU-cost comparison.

`rust-raknet` uses a nominal MTU of 1,428 B here, including 28 B of IPv4/UDP
headers. C KCP uses a 1,400 B UDP MTU, excluding those headers. Their IPv4 packet
budgets match, and both split a 4,096 B message into three fragments. The default
`rust-raknet` MTU is still 1,400 B, which needs four fragments for that message.
C KCP uses message mode, send/receive windows of 64/128 segments,
`nodelay(1, 10, 2, 1)`, immediate writes/ACKs, and no FEC or encryption.
`rust-raknet` keeps its normal flight window and retry timings. quic-go uses a
1,400 B UDP packet size with path MTU discovery disabled, matching the 1,428 B
IPv4 budget. QUIC retains its normal congestion and flow control settings.

All four implementations completed 18/18 throughput runs each. `rust-raknet`
leads C KCP in five profiles; with delay and loss, they are effectively tied. TCP
leads the 4 KiB profile, and quic-go leads the 64 B profile.

</details>

### Single-connection latency

RTT is round-trip latency. **Lower is better.** Values are the medians of
run-level p50 / p95 / p99 percentiles from three runs, each with 300 warmups and
3,000 sequential samples.

| Network profile | TCP RTT (p50 / p95 / p99) | rust-raknet RTT (p50 / p95 / p99) | C KCP RTT (p50 / p95 / p99) | quic-go RTT (p50 / p95 / p99) |
| --- | ---: | ---: | ---: | ---: |
| No injected loss, CPU affinity | 16.1 µs / 75.2 µs / 87.1 µs | 17.6 µs / 22.4 µs / 48.0 µs | 13.8 µs / 57.5 µs / 62.0 µs | 62.7 µs / 99.9 µs / 140.9 µs |

Client and server are pinned to separate CPUs. Each Rust process has four workers
on its assigned CPU; quic-go uses `GOMAXPROCS=4` on its assigned CPU, and C KCP
has one event loop. These samples precede the burst and do not measure delivery
latency under sustained load.

### Concurrent connections

800 B messages, a sliding window of 16 per connection, and **1,048,576 verified
ordered echoes** per run. All four implementations use four workers and four
server sockets with `SO_REUSEPORT`, with separate sets of four client/server CPUs.
These are transport sessions, not authenticated Minecraft players.

| Connections | Injected loss | TCP | **rust-raknet** | C KCP | quic-go |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 64 connections | 0% | 713.62 MiB/s | 543.43 MiB/s | 362.82 MiB/s | 220.49 MiB/s |
| 256 connections | 0% | 598.00 MiB/s | 556.90 MiB/s | 415.87 MiB/s | 184.68 MiB/s |
| 1,024 connections | 0% | 591.75 MiB/s | 497.71 MiB/s | 380.07 MiB/s | 164.48 MiB/s |
| 2,048 connections | 0% | 586.90 MiB/s | 440.64 MiB/s | 361.93 MiB/s | 133.22 MiB/s |
| 64 connections | 1% | 390.96 MiB/s | 520.11 MiB/s | 377.12 MiB/s | 216.83 MiB/s |
| 256 connections | 1% | 519.37 MiB/s | 511.55 MiB/s | 384.26 MiB/s | 147.19 MiB/s |
| 1,024 connections | 1% | 424.33 MiB/s | 455.86 MiB/s | 390.54 MiB/s | 64.55 MiB/s |
| 2,048 connections | 1% | 423.78 MiB/s | 389.14 MiB/s | 335.09 MiB/s | 31.48 MiB/s |

TCP leads all four clean groups. At 1% loss, `rust-raknet` leads TCP at 64 and
1,024 connections; TCP leads at 256 and 2,048. `rust-raknet` leads C KCP in all
eight groups. Small differences with overlapping ranges need more runs before
calling a winner.

<details>
<summary>Concurrent throughput ranges and setup</summary>

| Connections | Injected loss | TCP throughput (median, range) | rust-raknet throughput (median, range) | C KCP throughput (median, range) | quic-go throughput (median, range) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 64 connections | 0% | 713.62 MiB/s (700.90–714.45 MiB/s) | 543.43 MiB/s (536.83–572.64 MiB/s) | 362.82 MiB/s (361.95–377.37 MiB/s) | 220.49 MiB/s (217.91–220.77 MiB/s) |
| 256 connections | 0% | 598.00 MiB/s (581.63–620.48 MiB/s) | 556.90 MiB/s (526.97–562.89 MiB/s) | 415.87 MiB/s (329.26–424.58 MiB/s) | 184.68 MiB/s (178.88–186.38 MiB/s) |
| 1,024 connections | 0% | 591.75 MiB/s (562.39–594.38 MiB/s) | 497.71 MiB/s (466.01–523.55 MiB/s) | 380.07 MiB/s (378.09–415.04 MiB/s) | 164.48 MiB/s (158.35–166.63 MiB/s) |
| 2,048 connections | 0% | 586.90 MiB/s (579.19–593.85 MiB/s) | 440.64 MiB/s (429.02–452.66 MiB/s) | 361.93 MiB/s (310.89–376.98 MiB/s) | 133.22 MiB/s (129.02–135.82 MiB/s) |
| 64 connections | 1% | 390.96 MiB/s (388.32–424.96 MiB/s) | 520.11 MiB/s (491.58–583.39 MiB/s) | 377.12 MiB/s (358.86–406.28 MiB/s) | 216.83 MiB/s (212.02–220.87 MiB/s) |
| 256 connections | 1% | 519.37 MiB/s (440.41–549.12 MiB/s) | 511.55 MiB/s (462.67–513.39 MiB/s) | 384.26 MiB/s (372.07–406.03 MiB/s) | 147.19 MiB/s (138.19–170.24 MiB/s) |
| 1,024 connections | 1% | 424.33 MiB/s (416.75–478.65 MiB/s) | 455.86 MiB/s (424.03–468.14 MiB/s) | 390.54 MiB/s (372.44–415.87 MiB/s) | 64.55 MiB/s (45.63–81.04 MiB/s) |
| 2,048 connections | 1% | 423.78 MiB/s (114.57–465.28 MiB/s) | 389.14 MiB/s (360.29–395.85 MiB/s) | 335.09 MiB/s (323.29–356.21 MiB/s) | 31.48 MiB/s (29.59–35.89 MiB/s) |

Order rotates across TCP / `rust-raknet` / C KCP. Each
connection completes 20 sequential RTT samples before a shared start barrier.
Setup and those samples are outside the throughput timer; connections stay open
until every burst finishes. All three implementations completed 24/24 runs each
(72/72 total). The separate quic-go batch completed 24/24 concurrent runs with
the same message counts, 16-message window, CPU affinity and start barriers.

TCP uses `TCP_NODELAY`, complete-record writes and reused buffers. Its four
listeners distribute accepts; accepted streams each have their own socket. UDP
sockets distribute datagrams. `rust-raknet` uses the default nominal MTU of
1,400 B; C KCP uses a 1,400 B UDP MTU. Their IPv4 budgets differ by 28 B, but an
800 B message fits one datagram in both. TCP can combine several records in a
segment, so packet loss percentages do not imply equal lost-message counts.
quic-go uses a 1,372 B UDP packet size and disables path MTU discovery, matching
the default 1,400 B `rust-raknet` IPv4 budget. It uses four `SO_REUSEPORT` receive
sockets and `GOMAXPROCS=4` per process. Connections keep one stream open each.

The 2,048-connection TCP loss case spans 114.57–465.28 MiB/s. Random loss affects
data and ACKs in both directions, and each run has a different trace. UDP buffers
can also overflow under saturation, even at 0% injected loss. Check UDP error
counters alongside netem counters when reproducing these results. The quic-go
batch also recorded receive-buffer drops at 2,048 connections without injected
loss; its 64- and 256-connection clean runs had none. CPU affinity does not
reserve the CPUs exclusively.

</details>

Build instructions for the repository TCP/RakNet drivers and workload details are in
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
