<div align="center">

# rust-raknet

**English** | [简体中文](README.zh-CN.md)

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

### Forwarding owned buffers

`recv_bytes()` and `send_bytes()` let a relay share an immutable payload with
its send and retry queues instead of copying the application bytes again:

```rust
use rust_raknet::{RaknetSocket, Reliability, error::Result};

async fn forward(source: &RaknetSocket, destination: &RaknetSocket) -> Result<()> {
    loop {
        let payload = source.recv_bytes().await?;
        destination.send_bytes(payload, Reliability::ReliableOrdered).await?;
    }
}
```

`send_bytes_with_order_channel()` selects an ordering channel. The existing
slice-based send and `Vec<u8>` receive APIs remain available. Both paths use the
same queue limits, fragmentation and delivery guarantees.

### Batching ready messages

`send_batch` and `send_bytes_batch` pack small `ReliableOrdered` messages into
standard RakNet frame sets, without waiting to collect more messages. Other
modes and fragmented messages stay in separate datagrams. Batches that cannot
fit two messages in one packet retain the ordinary sending path. A datagram
contains at most eight messages to limit the impact of a lost packet. Loss
feedback or a retransmission timeout switches to individual sends for the
rest of that connection. Each
message keeps its own delivery indexes and ordering channel; a datagram ACK
confirms every reliable member.

```rust
use rust_raknet::{Bytes, RaknetSocket, Reliability};

async fn forward(
    source: &RaknetSocket,
    target: &RaknetSocket,
) -> rust_raknet::error::Result<()> {
    let mut messages = Vec::<Bytes>::with_capacity(16);
    loop {
        source.recv_bytes_batch(&mut messages, 16).await?;
        target.send_bytes_batch(&messages, Reliability::ReliableOrdered).await?;
    }
}
```

Receive batches clear and reuse the supplied vector and contain at most 64
messages. Channel-specific sending is available through
`send_batch_with_order_channel` and `send_bytes_batch_with_order_channel`.
Normal `send` calls still send immediately. All batch members are validated
before any are queued; cancelling a send may leave a prefix queued for delivery.

### Example layout

All examples live under `examples/`. Single-file programs run with
`cargo run --example NAME`. The `proxy`, `bedrock_ping` and `test_benchmark`
subdirectories are standalone Cargo projects; run them with
`cargo run --manifest-path examples/PROJECT/Cargo.toml`.

## Minecraft Bedrock

The crate transports Bedrock packets as bytes. It does not implement Xbox login
or decode the game protocol. The [proxy example](examples/proxy) forwards RakNet UDP
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
`listen()`. In `examples/proxy/src/main.rs`, insert this before
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
cargo run --release --manifest-path examples/proxy/Cargo.toml -- \
  -l 127.0.0.1:19144 -r 127.0.0.1:19142
```

For a local test, add `127.0.0.1:19144` to the client's server list. On another
machine, use the proxy's reachable address and frontend port. The client must
join the proxy endpoint for its game traffic to pass through `rust-raknet`.

The proxy keeps the client's RakNet version when connecting upstream. Both
forwarding directions run concurrently; upstream handshakes time out after 10 s.
On Linux, add `--socket-shards 4` to opt into four receive sockets. The default is
one; several frontend shards can be slower when they feed a single upstream socket.
Add `--batch-messages` to forward already-ready messages through the explicit
batch API. This is opt-in and never waits to fill a batch.

### Server discovery

```sh
cargo run --manifest-path examples/bedrock_ping/Cargo.toml -- play.example.com:19132
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

For sustained traffic, Linux listeners can also opt into
`listener.with_receive_batching(true)` before `listen()`. This receives up to
16 already-ready datagrams per system call; it does not wait to fill a batch.
The option applies to all shards and defaults to off because sparse traffic
can have higher processing latency with batch reception. Compare your workload
before enabling it. The sharded echo example accepts `--receive-batching` as
an optional argument.

For servers with many idle peers, `listener.with_idle_maintenance(true)` pauses
periodic maintenance after 500 ms without incoming traffic and with empty
queues. New work resumes maintenance. Connected clients can use
`socket.set_idle_maintenance(true)` and switch it off at runtime. This option
is also off by default: it saves idle CPU, but can change latency under load.
Measure both busy and quiet traffic before enabling it. The sharded echo
example accepts `--idle-maintenance`.

</details>

<details>
<summary><b>Queues, receive limits and shutdown</b></summary>

- Sends wait for the connected handshake and for space in a full send queue.
  Reliable delivery allows at most 64 frames in flight, including fragments. A normal burst queues up to
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

These measurements compare this **rust-raknet implementation**, using either
ordinary sends or explicit batch APIs, with TCP, the official C KCP core and
quic-go. They do not measure the RakNet protocol in general.

**Measured on October 2, 2026.** Intel Core i7-9700F (8 logical CPUs), Linux
x86_64, Rust 1.98.1, Tokio 1.53.1, GCC 13.3.0 and Go 1.27.1; optimized builds.
All processes ran in a task copy with private caches and a private loopback
network namespace. Loopback MTU was 1,500 B, GSO/GRO was limited to one packet,
and netem's queue limit was 100,000 packets. Processes used `nice 10`.
Host network settings were unchanged.

### Implementations and API modes

| Column | GitHub source | Tested version / revision |
| --- | --- | --- |
| TCP | [TCP echo driver](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark), [Tokio](https://github.com/tokio-rs/tokio) | Driver 0.1.0; Tokio 1.53.1; Linux TCP stack 7.0.11-76070011-generic |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 0.16.0 working tree based on `a957256`, including pending performance changes |
| rust-raknet (batch APIs) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | The same library build as the `send` column |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | Pinned commit `b1a7a21`; upstream C core unchanged |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

- **`send`**: both endpoints use ordinary `send` / `recv`, with no batch API calls.
- **Batch APIs**: the single-connection client uses `send_batch`; the concurrent
  client uses `send_bytes_batch`. Echo servers use `recv_bytes_batch` and
  `send_bytes_batch`. Each call submits at most 16 already-ready messages;
  neither endpoint waits to fill a batch.

The concurrent batch client constructs owned payloads for its batches; the
ordinary client reuses a borrowed payload template. The comparison includes
buffer construction, send APIs and receive draining. It does not isolate the
cost or benefit of packet packing alone.

Each run starts fresh servers and connections. Both rust-raknet columns use
`ReliableOrdered`, the normal 64-frame reliable flight limit and normal retry
settings. Optional UDP receive batching and idle maintenance are disabled.
Message batching is separate from UDP syscall batching, which both API modes
can use.

A UDP datagram packs at most **eight** non-fragmented messages, within the MTU.
Two 800 B messages do not fit together at the tested MTUs; 4,096 B messages
are fragmented and are not packed. Those batch columns measure the batch API
and receive-draining path, not message coalescing. A matched NACK or reliable
retransmission timeout disables packing for the rest of that connection,
including loss seen during warmup. Thus **calling the batch API does not mean
that every run actually merges messages**.

TCP uses `TCP_NODELAY`, four-byte little-endian record lengths, complete-record
writes and reused buffers. C KCP uses message mode, segment windows of 64/128,
`nodelay(1, 10, 2, 1)`, immediate writes/ACKs, and no FEC or encryption.
QUIC uses one bidirectional reliable ordered stream per connection with the
same record framing as TCP. Encryption, congestion control and stream flow
control remain enabled; rust-raknet and C KCP do not provide the same features.

Throughput counts verified echoed application payload **per direction**, excluding
headers and ACKs. **Higher is better.** Tables show medians of three runs, with
rotating order across all five columns. 1 MiB = 1,048,576 B. Loss is independently
randomized per run and affects data and ACKs in both directions.

### Single connection

All clients keep at most 64 application messages awaiting echoes. Each run has
100 warmups and 300 sequential RTT samples before the measured burst. Rust and
Go use four workers; C KCP uses one event loop per process. Throughput runs are
not pinned to CPUs, so these are not equal-CPU-efficiency measurements.

| Network profile | Payload / burst count | TCP | rust-raknet (`send`) | rust-raknet (batch APIs) | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- |
| No injected loss | 800 B / 200,000 messages | 149.64 MiB/s | 188.84 MiB/s | 194.79 MiB/s | 150.33 MiB/s | 114.28 MiB/s |
| 1% loss | 800 B / 50,000 messages | 17.87 MiB/s | 148.62 MiB/s | 150.42 MiB/s | 130.82 MiB/s | 73.61 MiB/s |
| 5% loss | 800 B / 50,000 messages | 1.10 MiB/s | 138.58 MiB/s | 138.57 MiB/s | 104.44 MiB/s | 9.24 MiB/s |
| No injected loss | 64 B / 300,000 messages | 13.45 MiB/s | 16.49 MiB/s | 19.81 MiB/s | 12.45 MiB/s | 19.07 MiB/s |
| 1% loss | 64 B / 100,000 messages | 3.25 MiB/s | 14.82 MiB/s | 15.53 MiB/s | 11.23 MiB/s | 15.70 MiB/s |
| 5% loss | 64 B / 100,000 messages | 0.12 MiB/s | 14.81 MiB/s | 11.98 MiB/s | 10.10 MiB/s | 3.46 MiB/s |
| No injected loss | 4,096 B / 50,000 messages | 343.72 MiB/s | 313.39 MiB/s | 316.06 MiB/s | 255.39 MiB/s | 219.47 MiB/s |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.04 MiB/s | 3.02 MiB/s | 2.96 MiB/s | 2.97 MiB/s | 1.57 MiB/s |

<details>
<summary>Single-connection throughput ranges</summary>

| Network profile | Payload / burst count | TCP | rust-raknet (`send`) | rust-raknet (batch APIs) | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- |
| No injected loss | 800 B / 200,000 messages | 149.64 MiB/s (149.44–157.64 MiB/s) | 188.84 MiB/s (186.11–189.02 MiB/s) | 194.79 MiB/s (191.36–195.96 MiB/s) | 150.33 MiB/s (146.79–153.49 MiB/s) | 114.28 MiB/s (85.89–115.88 MiB/s) |
| 1% loss | 800 B / 50,000 messages | 17.87 MiB/s (14.64–25.13 MiB/s) | 148.62 MiB/s (148.36–150.92 MiB/s) | 150.42 MiB/s (125.54–166.60 MiB/s) | 130.82 MiB/s (125.84–149.80 MiB/s) | 73.61 MiB/s (64.52–87.73 MiB/s) |
| 5% loss | 800 B / 50,000 messages | 1.10 MiB/s (1.02–1.12 MiB/s) | 138.58 MiB/s (73.17–168.10 MiB/s) | 138.57 MiB/s (85.28–143.16 MiB/s) | 104.44 MiB/s (95.78–122.29 MiB/s) | 9.24 MiB/s (8.35–10.51 MiB/s) |
| No injected loss | 64 B / 300,000 messages | 13.45 MiB/s (13.18–14.03 MiB/s) | 16.49 MiB/s (16.47–16.58 MiB/s) | 19.81 MiB/s (16.45–35.17 MiB/s) | 12.45 MiB/s (12.39–12.70 MiB/s) | 19.07 MiB/s (13.44–31.32 MiB/s) |
| 1% loss | 64 B / 100,000 messages | 3.25 MiB/s (2.81–3.69 MiB/s) | 14.82 MiB/s (14.52–16.11 MiB/s) | 15.53 MiB/s (14.44–16.70 MiB/s) | 11.23 MiB/s (10.53–11.92 MiB/s) | 15.70 MiB/s (13.10–21.80 MiB/s) |
| 5% loss | 64 B / 100,000 messages | 0.12 MiB/s (0.12–0.14 MiB/s) | 14.81 MiB/s (10.86–15.24 MiB/s) | 11.98 MiB/s (6.26–13.47 MiB/s) | 10.10 MiB/s (9.99–10.23 MiB/s) | 3.46 MiB/s (2.88–3.82 MiB/s) |
| No injected loss | 4,096 B / 50,000 messages | 343.72 MiB/s (308.23–348.77 MiB/s) | 313.39 MiB/s (312.68–314.14 MiB/s) | 316.06 MiB/s (310.03–316.51 MiB/s) | 255.39 MiB/s (249.93–259.25 MiB/s) | 219.47 MiB/s (218.10–227.73 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.04 MiB/s (0.98–1.16 MiB/s) | 3.02 MiB/s (2.84–3.27 MiB/s) | 2.96 MiB/s (2.94–3.01 MiB/s) | 2.97 MiB/s (2.95–3.04 MiB/s) | 1.57 MiB/s (1.55–1.64 MiB/s) |

</details>

Single-connection UDP packet budgets match: rust-raknet's nominal MTU is 1,428 B,
including 28 B of IPv4/UDP overhead; C KCP and quic-go use a 1,400 B UDP packet
budget. QUIC path MTU discovery is disabled. Each 4,096 B message uses three
fragments in rust-raknet and C KCP. The library's default nominal MTU remains
1,400 B.

### Sparse request/echo latency

**Lower is better.** Values below are medians of run-level RTT percentiles from
five runs, each with 1,000 warmups and 10,000 sequential samples, using 800 B
messages and no injected loss. Client and server are pinned to different CPUs;
each Rust/Go process has four workers on its assigned CPU, while C KCP uses one
event loop. CPU affinity does not reserve the CPUs exclusively.

| Implementation / API | Median RTT | 95th-percentile RTT | 99th-percentile RTT |
| --- | --- | --- | --- |
| TCP | 12.8 µs | 20.6 µs | 37.9 µs |
| rust-raknet (`send`) | 17.6 µs | 22.7 µs | 47.9 µs |
| rust-raknet (batch APIs) | 17.4 µs | 22.2 µs | 47.0 µs |
| C KCP | 10.4 µs | 15.9 µs | 33.3 µs |
| quic-go | 61.9 µs | 94.6 µs | 129.6 µs |

The batch client calls `send_batch` with **one** message for these sparse samples;
the batch server likewise replies without waiting for more messages. No messages
are merged here. These results measure the sparse API path, not burst delivery
latency. Concurrent throughput runs also report pre-burst RTTs; they are not
used as loaded latency below.

### Concurrent throughput

Each run verifies **1,048,576 ordered echoes**, with a 16-message application
window per connection. Every echoed connection ID, message ID and payload is
checked. Twenty sequential samples per connection precede a common start
barrier; setup and those samples are excluded from the throughput timer.
Connections stay open until every burst finishes.

All implementations use four workers and four server listeners/receive sockets
with `SO_REUSEPORT`. Servers are pinned to four CPUs and clients to the other
four. These are synthetic transport sessions, not authenticated Minecraft players.

| Payload | Connections | Injected loss | TCP | rust-raknet (`send`) | rust-raknet (batch APIs) | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 connections | 0% | 634.89 MiB/s | 573.33 MiB/s | 574.74 MiB/s | 390.10 MiB/s | 231.72 MiB/s |
| 800 B | 256 connections | 0% | 602.00 MiB/s | 559.84 MiB/s | 580.24 MiB/s | 412.41 MiB/s | 189.96 MiB/s |
| 800 B | 1,024 connections | 0% | 577.50 MiB/s | 495.19 MiB/s | 456.42 MiB/s | 351.83 MiB/s | 161.41 MiB/s |
| 800 B | 2,048 connections | 0% | 560.90 MiB/s | 437.78 MiB/s | 317.01 MiB/s | 313.08 MiB/s | 127.58 MiB/s |
| 800 B | 64 connections | 1% | 404.40 MiB/s | 486.37 MiB/s | 537.64 MiB/s | 342.39 MiB/s | 190.13 MiB/s |
| 800 B | 256 connections | 1% | 492.03 MiB/s | 446.77 MiB/s | 476.44 MiB/s | 400.05 MiB/s | 178.44 MiB/s |
| 800 B | 1,024 connections | 1% | 490.61 MiB/s | 439.75 MiB/s | 454.97 MiB/s | 362.80 MiB/s | 72.49 MiB/s |
| 800 B | 2,048 connections | 1% | 480.96 MiB/s | 346.41 MiB/s | 353.56 MiB/s | 336.77 MiB/s | 38.47 MiB/s |
| 64 B | 64 connections | 0% | 68.60 MiB/s | 47.59 MiB/s | 129.31 MiB/s | 32.87 MiB/s | 135.46 MiB/s |
| 64 B | 1,024 connections | 0% | 55.39 MiB/s | 49.03 MiB/s | 148.73 MiB/s | 43.48 MiB/s | 110.25 MiB/s |
| 64 B | 64 connections | 1% | 46.47 MiB/s | 44.86 MiB/s | 46.56 MiB/s | 32.54 MiB/s | 66.16 MiB/s |
| 64 B | 1,024 connections | 1% | 47.38 MiB/s | 42.28 MiB/s | 46.91 MiB/s | 43.08 MiB/s | 80.70 MiB/s |

<details>
<summary>Concurrent throughput ranges</summary>

| Payload | Connections | Injected loss | TCP | rust-raknet (`send`) | rust-raknet (batch APIs) | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 connections | 0% | 634.89 MiB/s (604.49–691.88 MiB/s) | 573.33 MiB/s (549.30–578.24 MiB/s) | 574.74 MiB/s (573.26–583.02 MiB/s) | 390.10 MiB/s (331.71–391.23 MiB/s) | 231.72 MiB/s (224.07–232.60 MiB/s) |
| 800 B | 256 connections | 0% | 602.00 MiB/s (599.13–632.16 MiB/s) | 559.84 MiB/s (515.24–567.53 MiB/s) | 580.24 MiB/s (553.57–585.65 MiB/s) | 412.41 MiB/s (410.23–418.18 MiB/s) | 189.96 MiB/s (189.17–194.65 MiB/s) |
| 800 B | 1,024 connections | 0% | 577.50 MiB/s (455.40–585.45 MiB/s) | 495.19 MiB/s (490.50–506.24 MiB/s) | 456.42 MiB/s (406.09–509.81 MiB/s) | 351.83 MiB/s (302.39–407.08 MiB/s) | 161.41 MiB/s (148.02–168.02 MiB/s) |
| 800 B | 2,048 connections | 0% | 560.90 MiB/s (485.08–571.37 MiB/s) | 437.78 MiB/s (397.60–440.29 MiB/s) | 317.01 MiB/s (295.58–443.27 MiB/s) | 313.08 MiB/s (295.83–346.13 MiB/s) | 127.58 MiB/s (81.90–137.95 MiB/s) |
| 800 B | 64 connections | 1% | 404.40 MiB/s (375.90–484.43 MiB/s) | 486.37 MiB/s (449.86–503.03 MiB/s) | 537.64 MiB/s (519.54–558.08 MiB/s) | 342.39 MiB/s (325.80–373.99 MiB/s) | 190.13 MiB/s (185.66–197.30 MiB/s) |
| 800 B | 256 connections | 1% | 492.03 MiB/s (490.61–544.03 MiB/s) | 446.77 MiB/s (374.17–495.70 MiB/s) | 476.44 MiB/s (472.61–514.43 MiB/s) | 400.05 MiB/s (390.22–420.57 MiB/s) | 178.44 MiB/s (177.19–178.79 MiB/s) |
| 800 B | 1,024 connections | 1% | 490.61 MiB/s (325.94–510.10 MiB/s) | 439.75 MiB/s (430.81–468.65 MiB/s) | 454.97 MiB/s (431.67–458.71 MiB/s) | 362.80 MiB/s (358.80–382.40 MiB/s) | 72.49 MiB/s (60.68–111.68 MiB/s) |
| 800 B | 2,048 connections | 1% | 480.96 MiB/s (437.30–493.70 MiB/s) | 346.41 MiB/s (336.19–367.02 MiB/s) | 353.56 MiB/s (328.69–389.72 MiB/s) | 336.77 MiB/s (319.42–346.90 MiB/s) | 38.47 MiB/s (33.38–46.39 MiB/s) |
| 64 B | 64 connections | 0% | 68.60 MiB/s (67.87–73.23 MiB/s) | 47.59 MiB/s (47.32–47.99 MiB/s) | 129.31 MiB/s (113.26–157.07 MiB/s) | 32.87 MiB/s (28.76–35.58 MiB/s) | 135.46 MiB/s (133.70–137.00 MiB/s) |
| 64 B | 1,024 connections | 0% | 55.39 MiB/s (54.94–55.73 MiB/s) | 49.03 MiB/s (47.57–51.86 MiB/s) | 148.73 MiB/s (124.90–149.36 MiB/s) | 43.48 MiB/s (40.99–43.67 MiB/s) | 110.25 MiB/s (108.30–111.87 MiB/s) |
| 64 B | 64 connections | 1% | 46.47 MiB/s (43.87–54.31 MiB/s) | 44.86 MiB/s (44.67–46.52 MiB/s) | 46.56 MiB/s (46.21–47.98 MiB/s) | 32.54 MiB/s (31.50–34.95 MiB/s) | 66.16 MiB/s (63.31–66.84 MiB/s) |
| 64 B | 1,024 connections | 1% | 47.38 MiB/s (44.98–51.74 MiB/s) | 42.28 MiB/s (42.08–42.58 MiB/s) | 46.91 MiB/s (45.06–47.80 MiB/s) | 43.08 MiB/s (35.78–43.10 MiB/s) | 80.70 MiB/s (76.90–83.43 MiB/s) |

</details>

Concurrent runs use rust-raknet's default nominal MTU of 1,400 B. quic-go's UDP
budget is 1,372 B, matching the 1,400 B IPv4 packet budget, with path MTU discovery
disabled. C KCP uses a 1,400 B UDP MTU; its physical budget is 28 B larger, but
both tested payload sizes fit without fragmentation. TCP may combine records
in a segment, so equal packet-loss percentages do not imply equal lost-message
counts across implementations.

### Delivery latency during sustained load

A separate instrumented driver timestamps messages before sending and measures
RTT when their verified echoes arrive throughout the burst. It uses the same
16-message window, worker counts, CPU placement and server modes, but verifies
262,144 burst echoes per run. These latency measurements include client and
server queueing. Instrumentation adds overhead, so its throughput is not mixed
into the tables above. Values are medians of three runs; each cell shows
**median RTT / 99th-percentile RTT**. Lower is better.

| Payload | Connections | Injected loss | TCP | rust-raknet (`send`) | rust-raknet (batch APIs) |
| --- | --- | --- | --- | --- | --- |
| 64 B | 64 connections | 0% | 0.94 ms / 5.28 ms | 1.00 ms / 7.84 ms | 0.45 ms / 3.71 ms |
| 64 B | 1,024 connections | 0% | 19.12 ms / 38.37 ms | 15.99 ms / 42.99 ms | 4.21 ms / 22.42 ms |
| 64 B | 64 connections | 1% | 0.65 ms / 3.74 ms | 0.94 ms / 5.83 ms | 0.95 ms / 4.85 ms |
| 64 B | 1,024 connections | 1% | 15.71 ms / 37.28 ms | 15.44 ms / 62.46 ms | 10.22 ms / 51.70 ms |
| 800 B | 64 connections | 0% | 1.15 ms / 9.62 ms | 1.10 ms / 8.51 ms | 1.11 ms / 9.22 ms |
| 800 B | 1,024 connections | 0% | 20.37 ms / 40.35 ms | 19.24 ms / 124.51 ms | 17.59 ms / 113.04 ms |
| 800 B | 64 connections | 1% | 0.66 ms / 3.95 ms | 0.99 ms / 5.90 ms | 1.05 ms / 5.92 ms |
| 800 B | 1,024 connections | 1% | 18.34 ms / 48.50 ms | 18.23 ms / 71.80 ms | 20.38 ms / 100.26 ms |

### What these runs show

- For 64 B messages with no injected loss, batch APIs improve concurrent
  throughput from 47.59 to 129.31 MiB/s at 64 connections and from 49.03 to
  148.73 MiB/s at 1,024 connections. In the separate loaded-latency measurement
  at 1,024 connections, median RTT falls from 15.99 to 4.21 ms and the
  99th-percentile RTT from 42.99 to 22.42 ms.
- The single-connection 64 B batch result is less stable: 19.81 MiB/s median,
  with a 16.45–35.17 MiB/s range. It does not consistently reproduce the previous
  batch's small-message gain. A separate diagnostic build observed matched NACKs
  disabling packing in four of ten no-injected-loss runs, despite zero netem
  drops and zero UDP receive-buffer drops. A NACK reports a sequence gap; it
  does not by itself prove permanent packet loss. The current permanent fallback
  makes batching sensitive to that feedback. Diagnostic results are excluded
  from the tables.
- Ordinary rust-raknet sends retain strong single-connection 800 B throughput,
  including the 1% and 5% loss profiles. TCP leads the 4 KiB single-connection
  profile and the 800 B concurrent groups without injected loss. quic-go leads
  the 64 B / 64-connection group and both 64 B concurrent loss groups. There is
  no winner across every workload.
- Batch APIs are **not a general default-speedup switch**. At 800 B and 2,048
  connections without injected loss, they measure 317.01 MiB/s versus
  437.78 MiB/s for ordinary sends, about 28% lower. At 64 B and 5% loss on one
  connection, they measure 11.98 versus 14.81 MiB/s, about 19% lower. The
  API/draining and driver-buffer differences still matter when packing is
  unavailable or has been disabled.
- Sparse median RTT is similar between the two API modes: 17.6 µs for `send`
  and 17.4 µs for batch APIs. Loaded latency is not uniformly better: at 800 B,
  1,024 connections and 1% loss, the batch mode's 99th-percentile RTT is
  100.26 ms versus 71.80 ms for ordinary sends. Select the mode using the
  payload sizes, concurrency and latency distribution of the actual workload.


Random netem loss and namespace-local UDP receive-buffer errors were recorded
for every run. Saturated UDP sockets can drop packets even with no injected
loss. Maximum receive-buffer drops in one throughput run: rust-raknet (`send`): 187,997 datagrams, rust-raknet (batch APIs): 375,102 datagrams, C KCP: 335,992 datagrams, quic-go: 6,124 datagrams.
These are part of the measured workload. Random loss and shared CPU scheduling
make the ranges relevant; a small median difference is not a universal ranking.

All 397 measurements completed with verified echoes: 120 single-connection
throughput runs, 25 sparse-latency runs, 180 concurrent-throughput runs and
72 loaded-latency runs. This batch replaces the previous tables; it is not a
controlled before/after comparison with an older library revision.

Build commands, API flags and workload details are in
[the benchmark README](examples/test_benchmark/README.md) and
[the C KCP adapters](examples/test_benchmark/kcp/README.md).

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
dependency snippets in both READMEs and the crate documentation together. CI checks they match.

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
