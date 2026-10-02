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
rust-raknet = "1.0.0"
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
`rust-raknet` 1.0.0 over RakNet UDP.

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
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 0.16.0 at `d65aee3`; benchmark instrumentation added |
| rust-raknet (batch APIs) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | The same library build as the `send` column |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | Pinned commit `b1a7a21`; upstream C core unchanged |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

- **`send`**: both endpoints use ordinary `send` / `recv`, with no batch API calls.
- **Batch APIs**: the single-connection client uses `send_batch`; the concurrent
  client uses `send_bytes_batch`. Echo servers use `recv_bytes_batch` and
  `send_bytes_batch`. Each call submits at most 16 already-ready messages;
  neither endpoint waits to fill a batch.

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
writes and reused buffers. C KCP uses message mode, send/receive segment windows
of 64/64 for single-connection runs and 64/128 for concurrent runs,
`nodelay(1, 10, 2, 1)`, immediate writes/ACKs, and no FEC or encryption.
QUIC uses one bidirectional reliable ordered stream per connection with the
same record framing as TCP. Encryption, congestion control and stream flow
control remain enabled; rust-raknet and C KCP do not provide the same features.

Throughput counts verified echoed application payload **per direction**, excluding
headers and ACKs. **Higher is better.** Tables show medians of three runs, with
rotating order across all five columns. 1 MiB = 1,048,576 B. Loss is independently
randomized per run and affects data and ACKs in both directions.

Values are medians across repetitions; parentheses show the min–max range.
Every value includes its unit. Throughput cells list throughput, then peak RSS
for the client (**C**) and server (**S**). Latency tables include ranges and
memory from their own measured workload.

Linux `wait4` records whole-process peak RSS across startup, setup, warmup, load
and teardown. This includes runtime, allocator and application buffers, excluding
kernel socket buffers. TCP transport state lives in the kernel, so RSS is not
total network memory. C / S peaks are measured separately, need not occur
together, and are not per-connection footprints.

### Single connection

All clients keep at most 64 application messages awaiting echoes. Each run has
100 warmups and 300 sequential RTT samples before the measured burst. Rust and
Go use four workers; C KCP uses one event loop per process. Throughput runs are
not pinned to CPUs, so these are not equal-CPU-efficiency measurements.

| Network profile | Payload / burst count | TCP<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>Throughput (range)<br>RSS: C / S | C KCP<br>Throughput (range)<br>RSS: C / S | quic-go<br>Throughput (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- |
| No injected loss | 800 B / 200,000 messages | 150.71 MiB/s (149.60–151.39 MiB/s)<br>C: 3.53 MiB (3.46–3.54 MiB)<br>S: 3.45 MiB (3.37–3.45 MiB) | 188.34 MiB/s (183.93–191.66 MiB/s)<br>C: 3.84 MiB (3.69–3.98 MiB)<br>S: 3.77 MiB (3.60–3.89 MiB) | 193.30 MiB/s (189.46–195.30 MiB/s)<br>C: 3.85 MiB (3.71–3.99 MiB)<br>S: 3.88 MiB (3.67–4.00 MiB) | 152.98 MiB/s (151.66–155.92 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 116.06 MiB/s (115.70–117.91 MiB/s)<br>C: 14.15 MiB (14.14–14.41 MiB)<br>S: 13.70 MiB (13.55–14.21 MiB) |
| 1% loss | 800 B / 50,000 messages | 17.02 MiB/s (14.55–19.00 MiB/s)<br>C: 3.65 MiB (3.42–3.74 MiB)<br>S: 3.44 MiB (3.29–3.50 MiB) | 151.54 MiB/s (151.06–151.61 MiB/s)<br>C: 3.70 MiB (3.70–3.82 MiB)<br>S: 3.82 MiB (3.54–3.85 MiB) | 158.37 MiB/s (151.12–162.72 MiB/s)<br>C: 3.78 MiB (3.59–3.99 MiB)<br>S: 3.95 MiB (3.79–4.02 MiB) | 131.88 MiB/s (129.13–132.70 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 77.07 MiB/s (29.79–87.86 MiB/s)<br>C: 13.75 MiB (13.49–13.85 MiB)<br>S: 13.52 MiB (13.50–13.56 MiB) |
| 5% loss | 800 B / 50,000 messages | 1.06 MiB/s (0.99–1.11 MiB/s)<br>C: 3.55 MiB (3.41–3.77 MiB)<br>S: 3.41 MiB (3.29–3.45 MiB) | 146.94 MiB/s (140.16–149.13 MiB/s)<br>C: 3.85 MiB (3.81–3.91 MiB)<br>S: 3.78 MiB (3.39–3.98 MiB) | 142.99 MiB/s (133.51–144.10 MiB/s)<br>C: 3.83 MiB (3.72–4.10 MiB)<br>S: 3.84 MiB (3.74–3.86 MiB) | 113.61 MiB/s (111.54–124.69 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) | 10.03 MiB/s (8.66–13.69 MiB/s)<br>C: 13.68 MiB (13.64–13.78 MiB)<br>S: 14.00 MiB (13.89–14.31 MiB) |
| No injected loss | 64 B / 300,000 messages | 13.82 MiB/s (13.32–14.09 MiB/s)<br>C: 3.58 MiB (3.41–3.75 MiB)<br>S: 3.40 MiB (3.29–3.49 MiB) | 16.45 MiB/s (16.25–16.66 MiB/s)<br>C: 3.58 MiB (3.50–3.79 MiB)<br>S: 3.70 MiB (3.69–3.78 MiB) | 49.83 MiB/s (49.04–50.50 MiB/s)<br>C: 3.58 MiB (3.57–3.73 MiB)<br>S: 3.71 MiB (3.52–3.73 MiB) | 12.56 MiB/s (12.43–12.72 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 31.30 MiB/s (10.36–31.48 MiB/s)<br>C: 13.51 MiB (13.25–13.89 MiB)<br>S: 13.51 MiB (13.32–14.20 MiB) |
| 1% loss | 64 B / 100,000 messages | 2.34 MiB/s (1.63–3.86 MiB/s)<br>C: 3.58 MiB (3.36–3.63 MiB)<br>S: 3.34 MiB (3.25–3.51 MiB) | 15.13 MiB/s (14.65–16.43 MiB/s)<br>C: 3.86 MiB (3.73–3.89 MiB)<br>S: 3.80 MiB (3.70–3.83 MiB) | 14.91 MiB/s (14.85–16.35 MiB/s)<br>C: 3.73 MiB (3.63–3.89 MiB)<br>S: 3.71 MiB (3.68–3.74 MiB) | 11.45 MiB/s (11.34–11.72 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 13.69 MiB/s (8.00–21.54 MiB/s)<br>C: 12.64 MiB (12.01–12.76 MiB)<br>S: 11.95 MiB (11.57–12.07 MiB) |
| 5% loss | 64 B / 100,000 messages | 0.11 MiB/s (0.11–0.13 MiB/s)<br>C: 3.58 MiB (3.46–3.63 MiB)<br>S: 3.35 MiB (3.33–3.50 MiB) | 13.42 MiB/s (13.26–13.63 MiB/s)<br>C: 3.62 MiB (3.59–3.62 MiB)<br>S: 3.66 MiB (3.48–3.77 MiB) | 13.92 MiB/s (13.91–14.32 MiB/s)<br>C: 3.79 MiB (3.78–3.82 MiB)<br>S: 3.81 MiB (3.60–3.89 MiB) | 10.16 MiB/s (9.96–10.19 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 3.15 MiB/s (2.73–3.20 MiB/s)<br>C: 12.64 MiB (12.51–12.76 MiB)<br>S: 12.13 MiB (12.07–12.20 MiB) |
| No injected loss | 4,096 B / 50,000 messages | 345.98 MiB/s (342.74–347.49 MiB/s)<br>C: 3.59 MiB (3.52–3.61 MiB)<br>S: 3.45 MiB (3.43–3.49 MiB) | 314.30 MiB/s (301.29–315.82 MiB/s)<br>C: 4.71 MiB (4.43–4.76 MiB)<br>S: 4.68 MiB (4.64–4.76 MiB) | 313.17 MiB/s (310.92–315.55 MiB/s)<br>C: 4.66 MiB (4.65–4.89 MiB)<br>S: 4.80 MiB (4.67–5.02 MiB) | 256.66 MiB/s (249.47–263.40 MiB/s)<br>C: 1.87 MiB (1.86–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 227.94 MiB/s (149.56–228.93 MiB/s)<br>C: 14.23 MiB (14.18–14.24 MiB)<br>S: 13.73 MiB (13.72–14.10 MiB) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.21 MiB/s (1.09–1.28 MiB/s)<br>C: 3.58 MiB (3.56–3.59 MiB)<br>S: 3.34 MiB (3.26–3.45 MiB) | 2.91 MiB/s (2.72–2.96 MiB/s)<br>C: 3.80 MiB (3.78–3.98 MiB)<br>S: 3.68 MiB (3.64–3.70 MiB) | 3.05 MiB/s (2.99–3.12 MiB/s)<br>C: 3.79 MiB (3.61–3.89 MiB)<br>S: 3.71 MiB (3.65–3.80 MiB) | 2.85 MiB/s (2.79–2.95 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 1.67 MiB/s (1.55–1.79 MiB/s)<br>C: 11.83 MiB (11.64–11.89 MiB)<br>S: 11.45 MiB (10.70–11.57 MiB) |

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

| Implementation / API | Median RTT (range) | 95th-percentile RTT (range) | 99th-percentile RTT (range) | Peak RSS: C / S (range) |
| --- | --- | --- | --- | --- |
| TCP | 13.0 µs (12.8–13.1 µs) | 19.8 µs (19.5–21.6 µs) | 37.3 µs (37.1–39.5 µs) | C: 3.87 MiB (3.84–3.98 MiB)<br>S: 3.47 MiB (3.45–3.48 MiB) |
| rust-raknet (`send`) | 18.5 µs (17.2–18.6 µs) | 24.4 µs (22.6–32.9 µs) | 48.9 µs (45.0–53.3 µs) | C: 4.31 MiB (4.20–4.36 MiB)<br>S: 4.12 MiB (3.90–4.25 MiB) |
| rust-raknet (batch APIs) | 18.2 µs (17.4–18.4 µs) | 22.4 µs (21.7–25.2 µs) | 47.7 µs (45.9–49.3 µs) | C: 4.16 MiB (4.15–4.39 MiB)<br>S: 4.01 MiB (3.90–4.08 MiB) |
| C KCP | 10.4 µs (10.4–10.5 µs) | 16.0 µs (14.2–16.3 µs) | 32.6 µs (27.1–35.2 µs) | C: 1.88 MiB (1.87–1.88 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) |
| quic-go | 62.0 µs (61.9–62.1 µs) | 96.7 µs (95.7–98.0 µs) | 132.6 µs (126.2–170.2 µs) | C: 13.26 MiB (12.95–13.57 MiB)<br>S: 11.01 MiB (10.95–11.07 MiB) |

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
four. These measurements use reliable ordered echo workloads.

| Payload | Connections | Injected loss | TCP<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>Throughput (range)<br>RSS: C / S | C KCP<br>Throughput (range)<br>RSS: C / S | quic-go<br>Throughput (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 connections | 0% | 686.84 MiB/s (660.28–696.38 MiB/s)<br>C: 3.96 MiB (3.94–3.96 MiB)<br>S: 3.48 MiB (3.29–3.50 MiB) | 566.35 MiB/s (513.44–608.05 MiB/s)<br>C: 7.19 MiB (6.83–7.57 MiB)<br>S: 8.07 MiB (7.33–8.23 MiB) | 570.96 MiB/s (536.42–613.69 MiB/s)<br>C: 7.25 MiB (7.23–7.62 MiB)<br>S: 7.61 MiB (7.44–7.97 MiB) | 385.96 MiB/s (381.21–415.65 MiB/s)<br>C: 3.00 MiB (3.00–3.00 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 230.52 MiB/s (223.93–232.42 MiB/s)<br>C: 29.70 MiB (29.16–29.77 MiB)<br>S: 33.91 MiB (31.39–36.24 MiB) |
| 800 B | 256 connections | 0% | 607.51 MiB/s (573.21–608.02 MiB/s)<br>C: 5.78 MiB (5.64–5.78 MiB)<br>S: 3.66 MiB (3.53–3.72 MiB) | 549.78 MiB/s (546.04–569.34 MiB/s)<br>C: 15.78 MiB (15.75–15.90 MiB)<br>S: 14.38 MiB (13.83–14.53 MiB) | 549.17 MiB/s (526.89–561.53 MiB/s)<br>C: 16.47 MiB (16.31–16.75 MiB)<br>S: 14.93 MiB (14.28–15.23 MiB) | 407.50 MiB/s (388.71–408.66 MiB/s)<br>C: 6.25 MiB (6.25–6.25 MiB)<br>S: 7.00 MiB (6.88–7.00 MiB) | 191.40 MiB/s (188.11–193.07 MiB/s)<br>C: 77.63 MiB (76.96–77.87 MiB)<br>S: 79.33 MiB (72.77–81.14 MiB) |
| 800 B | 1,024 connections | 0% | 579.63 MiB/s (552.38–580.28 MiB/s)<br>C: 12.33 MiB (11.49–12.47 MiB)<br>S: 4.88 MiB (4.73–4.95 MiB) | 543.38 MiB/s (510.86–547.88 MiB/s)<br>C: 47.03 MiB (46.72–47.24 MiB)<br>S: 39.34 MiB (37.62–41.84 MiB) | 424.12 MiB/s (397.64–514.45 MiB/s)<br>C: 48.34 MiB (48.27–49.49 MiB)<br>S: 40.75 MiB (40.61–43.16 MiB) | 384.08 MiB/s (381.90–400.60 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 20.63 MiB (20.25–20.63 MiB) | 173.05 MiB/s (163.57–173.73 MiB/s)<br>C: 242.27 MiB (241.52–244.64 MiB)<br>S: 219.83 MiB (207.71–223.86 MiB) |
| 800 B | 2,048 connections | 0% | 551.95 MiB/s (515.72–587.01 MiB/s)<br>C: 18.20 MiB (16.71–20.39 MiB)<br>S: 6.61 MiB (6.57–6.68 MiB) | 433.48 MiB/s (430.64–434.75 MiB/s)<br>C: 87.66 MiB (87.63–89.96 MiB)<br>S: 69.42 MiB (69.22–70.91 MiB) | 408.05 MiB/s (367.75–414.65 MiB/s)<br>C: 89.64 MiB (89.14–91.14 MiB)<br>S: 74.59 MiB (73.68–74.95 MiB) | 342.55 MiB/s (334.12–344.45 MiB/s)<br>C: 38.75 MiB (38.63–38.88 MiB)<br>S: 38.38 MiB (37.63–38.38 MiB) | 136.72 MiB/s (135.05–138.96 MiB/s)<br>C: 465.14 MiB (459.12–467.06 MiB)<br>S: 380.00 MiB (367.71–399.96 MiB) |
| 800 B | 64 connections | 1% | 426.91 MiB/s (340.17–500.90 MiB/s)<br>C: 4.10 MiB (3.91–4.14 MiB)<br>S: 3.33 MiB (3.09–3.50 MiB) | 537.82 MiB/s (523.14–538.61 MiB/s)<br>C: 7.25 MiB (7.10–7.47 MiB)<br>S: 7.72 MiB (7.68–7.87 MiB) | 539.56 MiB/s (521.51–557.75 MiB/s)<br>C: 7.45 MiB (7.37–7.63 MiB)<br>S: 7.98 MiB (7.78–8.13 MiB) | 388.20 MiB/s (380.95–402.75 MiB/s)<br>C: 2.88 MiB (2.75–3.00 MiB)<br>S: 3.13 MiB (3.13–3.25 MiB) | 219.77 MiB/s (216.52–220.89 MiB/s)<br>C: 28.51 MiB (28.32–28.76 MiB)<br>S: 33.42 MiB (31.50–33.46 MiB) |
| 800 B | 256 connections | 1% | 478.63 MiB/s (460.11–516.55 MiB/s)<br>C: 5.80 MiB (5.73–5.87 MiB)<br>S: 3.64 MiB (3.53–3.70 MiB) | 474.69 MiB/s (473.86–497.07 MiB/s)<br>C: 16.32 MiB (16.14–17.25 MiB)<br>S: 15.69 MiB (15.27–15.75 MiB) | 503.83 MiB/s (499.65–505.25 MiB/s)<br>C: 17.13 MiB (16.84–17.18 MiB)<br>S: 15.37 MiB (15.36–15.56 MiB) | 390.50 MiB/s (327.94–418.43 MiB/s)<br>C: 6.25 MiB (6.25–6.38 MiB)<br>S: 6.88 MiB (6.75–6.88 MiB) | 183.33 MiB/s (182.52–185.20 MiB/s)<br>C: 75.93 MiB (75.65–76.08 MiB)<br>S: 72.21 MiB (71.33–73.33 MiB) |
| 800 B | 1,024 connections | 1% | 495.35 MiB/s (495.10–512.14 MiB/s)<br>C: 12.21 MiB (12.04–12.55 MiB)<br>S: 4.72 MiB (4.66–4.72 MiB) | 445.63 MiB/s (436.74–471.55 MiB/s)<br>C: 47.18 MiB (46.41–49.34 MiB)<br>S: 42.86 MiB (42.12–43.16 MiB) | 433.00 MiB/s (412.66–444.47 MiB/s)<br>C: 49.04 MiB (48.64–51.79 MiB)<br>S: 45.11 MiB (42.27–45.48 MiB) | 324.89 MiB/s (313.60–384.79 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 19.88 MiB (18.88–20.63 MiB) | 79.09 MiB/s (77.68–97.35 MiB/s)<br>C: 329.10 MiB (323.22–339.30 MiB)<br>S: 194.25 MiB (190.52–199.92 MiB) |
| 800 B | 2,048 connections | 1% | 425.19 MiB/s (255.63–440.72 MiB/s)<br>C: 17.40 MiB (17.23–20.09 MiB)<br>S: 6.60 MiB (6.43–6.64 MiB) | 371.13 MiB/s (341.29–387.26 MiB/s)<br>C: 90.00 MiB (88.92–92.36 MiB)<br>S: 74.96 MiB (73.77–77.34 MiB) | 324.86 MiB/s (323.00–328.90 MiB/s)<br>C: 92.60 MiB (92.43–92.98 MiB)<br>S: 77.47 MiB (75.64–77.80 MiB) | 300.43 MiB/s (270.36–316.49 MiB/s)<br>C: 38.75 MiB (38.75–39.00 MiB)<br>S: 37.38 MiB (36.13–38.00 MiB) | 36.02 MiB/s (34.86–38.96 MiB/s)<br>C: 760.16 MiB (753.87–775.20 MiB)<br>S: 411.05 MiB (403.68–418.38 MiB) |
| 64 B | 64 connections | 0% | 66.29 MiB/s (63.73–74.45 MiB/s)<br>C: 3.98 MiB (3.70–4.09 MiB)<br>S: 3.27 MiB (3.25–3.57 MiB) | 50.20 MiB/s (49.10–51.96 MiB/s)<br>C: 5.64 MiB (5.58–5.86 MiB)<br>S: 5.85 MiB (5.75–5.89 MiB) | 112.17 MiB/s (97.73–112.77 MiB/s)<br>C: 5.71 MiB (5.69–5.91 MiB)<br>S: 6.05 MiB (6.01–6.43 MiB) | 34.92 MiB/s (30.01–36.24 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.38–2.50 MiB) | 124.82 MiB/s (117.80–139.59 MiB/s)<br>C: 21.71 MiB (21.24–22.41 MiB)<br>S: 19.02 MiB (18.83–19.58 MiB) |
| 64 B | 1,024 connections | 0% | 55.28 MiB/s (54.29–56.05 MiB/s)<br>C: 9.24 MiB (9.09–9.31 MiB)<br>S: 4.14 MiB (4.12–4.36 MiB) | 48.66 MiB/s (47.83–50.19 MiB/s)<br>C: 32.75 MiB (32.27–32.95 MiB)<br>S: 27.39 MiB (25.48–28.52 MiB) | 142.12 MiB/s (140.84–146.14 MiB/s)<br>C: 32.24 MiB (31.98–32.27 MiB)<br>S: 25.79 MiB (25.66–25.85 MiB) | 42.03 MiB/s (41.17–44.17 MiB/s)<br>C: 8.88 MiB (8.88–8.88 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 109.34 MiB/s (108.98–109.42 MiB/s)<br>C: 147.46 MiB (131.64–148.71 MiB)<br>S: 96.83 MiB (94.08–96.85 MiB) |
| 64 B | 64 connections | 1% | 42.97 MiB/s (41.55–44.56 MiB/s)<br>C: 4.01 MiB (3.80–4.02 MiB)<br>S: 3.34 MiB (3.22–3.36 MiB) | 47.57 MiB/s (46.08–48.15 MiB/s)<br>C: 5.80 MiB (5.73–5.87 MiB)<br>S: 6.18 MiB (5.95–6.24 MiB) | 46.27 MiB/s (41.98–48.32 MiB/s)<br>C: 6.04 MiB (5.93–6.07 MiB)<br>S: 6.33 MiB (6.29–6.52 MiB) | 32.18 MiB/s (30.46–34.99 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.50 MiB (2.38–2.50 MiB) | 62.27 MiB/s (61.10–65.93 MiB/s)<br>C: 21.81 MiB (21.58–21.88 MiB)<br>S: 18.96 MiB (18.80–19.27 MiB) |
| 64 B | 1,024 connections | 1% | 51.20 MiB/s (46.05–53.09 MiB/s)<br>C: 9.13 MiB (9.09–9.58 MiB)<br>S: 4.09 MiB (3.91–4.11 MiB) | 43.49 MiB/s (42.68–43.57 MiB/s)<br>C: 34.34 MiB (33.05–35.57 MiB)<br>S: 29.52 MiB (28.13–29.59 MiB) | 49.85 MiB/s (49.79–52.47 MiB/s)<br>C: 35.72 MiB (35.22–35.84 MiB)<br>S: 30.05 MiB (29.84–30.09 MiB) | 40.47 MiB/s (37.55–42.08 MiB/s)<br>C: 8.75 MiB (8.75–8.75 MiB)<br>S: 9.25 MiB (9.13–9.25 MiB) | 81.37 MiB/s (81.09–90.10 MiB/s)<br>C: 132.64 MiB (130.52–142.89 MiB)<br>S: 95.26 MiB (93.71–96.21 MiB) |

Concurrent runs use rust-raknet's default nominal MTU of 1,400 B. quic-go's UDP
budget is 1,372 B, matching the 1,400 B IPv4 packet budget, with path MTU discovery
disabled. C KCP uses a 1,400 B UDP MTU; its physical budget is 28 B larger, but
both tested payload sizes fit without fragmentation. TCP may combine records
in a segment, so equal packet-loss percentages do not imply equal lost-message
counts across implementations.

### Delivery latency during sustained load

All concurrent clients support `--loaded-rtt`. They timestamp each message
before sending and measure
RTT when its verified echo is consumed throughout the burst. They use the same
16-message window, worker counts, CPU placement and server modes, but verify
262,144 burst echoes per run. These latency measurements include client and
server queueing. Instrumentation adds overhead, so its throughput is not mixed
into the tables above. Values are medians of three runs, showing
**median RTT and 99th-percentile RTT**, each with its range. Lower is better.
RSS includes the client's retained RTT samples, aggregation and sorting.

| Payload | Connections | Injected loss | TCP<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (`send`)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>RTT median / 99th (range)<br>RSS: C / S | C KCP<br>RTT median / 99th (range)<br>RSS: C / S | quic-go<br>RTT median / 99th (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 connections | 0% | 50%: 0.91 ms (0.88–0.95 ms)<br>99%: 3.66 ms (3.51–4.39 ms)<br>C: 11.54 MiB (11.46–11.61 MiB)<br>S: 3.28 MiB (3.22–3.32 MiB) | 50%: 1.04 ms (1.02–1.06 ms)<br>99%: 4.30 ms (3.39–8.22 ms)<br>C: 13.38 MiB (13.26–13.43 MiB)<br>S: 5.77 MiB (5.57–5.86 MiB) | 50%: 0.40 ms (0.37–0.43 ms)<br>99%: 3.99 ms (1.62–4.30 ms)<br>C: 13.38 MiB (13.28–13.46 MiB)<br>S: 5.90 MiB (5.74–5.99 MiB) | 50%: 1.32 ms (1.26–1.36 ms)<br>99%: 6.61 ms (5.66–9.60 ms)<br>C: 6.21 MiB (6.20–6.26 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.36 ms (0.35–0.36 ms)<br>99%: 2.49 ms (1.55–2.89 ms)<br>C: 30.30 MiB (27.44–31.44 MiB)<br>S: 17.33 MiB (17.21–17.71 MiB) |
| 64 B | 1,024 connections | 0% | 50%: 17.27 ms (17.04–17.89 ms)<br>99%: 36.27 ms (35.31–39.06 ms)<br>C: 16.73 MiB (16.35–17.03 MiB)<br>S: 4.13 MiB (4.00–4.23 MiB) | 50%: 15.99 ms (15.70–18.16 ms)<br>99%: 87.61 ms (36.52–94.68 ms)<br>C: 36.47 MiB (36.36–36.68 MiB)<br>S: 23.73 MiB (22.76–23.86 MiB) | 50%: 4.76 ms (4.53–5.00 ms)<br>99%: 19.49 ms (17.86–19.88 ms)<br>C: 37.91 MiB (37.82–38.02 MiB)<br>S: 24.41 MiB (24.10–24.41 MiB) | 50%: 5.12 ms (5.12–11.34 ms)<br>99%: 161.62 ms (101.00–162.49 ms)<br>C: 10.69 MiB (10.57–10.94 MiB)<br>S: 9.25 MiB (9.25–9.50 MiB) | 50%: 8.89 ms (8.67–8.94 ms)<br>99%: 14.09 ms (12.27–35.74 ms)<br>C: 138.33 MiB (138.08–139.46 MiB)<br>S: 86.21 MiB (84.71–92.96 MiB) |
| 64 B | 64 connections | 1% | 50%: 0.63 ms (0.59–0.68 ms)<br>99%: 3.41 ms (2.85–3.47 ms)<br>C: 11.62 MiB (11.61–11.82 MiB)<br>S: 3.43 MiB (3.39–3.46 MiB) | 50%: 0.94 ms (0.91–0.97 ms)<br>99%: 5.70 ms (5.18–5.81 ms)<br>C: 13.63 MiB (13.42–13.75 MiB)<br>S: 6.02 MiB (5.81–6.14 MiB) | 50%: 0.97 ms (0.91–0.99 ms)<br>99%: 5.12 ms (4.80–5.99 ms)<br>C: 13.71 MiB (13.69–13.92 MiB)<br>S: 6.18 MiB (6.11–6.41 MiB) | 50%: 1.23 ms (1.22–1.29 ms)<br>99%: 7.13 ms (5.39–8.05 ms)<br>C: 6.21 MiB (6.17–6.23 MiB)<br>S: 2.38 MiB (2.38–2.50 MiB) | 50%: 0.09 ms (0.09–0.10 ms)<br>99%: 27.22 ms (27.18–27.24 ms)<br>C: 33.52 MiB (30.96–34.39 MiB)<br>S: 17.46 MiB (16.96–17.57 MiB) |
| 64 B | 1,024 connections | 1% | 50%: 15.82 ms (14.79–16.82 ms)<br>99%: 35.74 ms (34.77–38.60 ms)<br>C: 17.36 MiB (16.71–17.39 MiB)<br>S: 4.25 MiB (4.14–4.25 MiB) | 50%: 16.12 ms (15.94–16.48 ms)<br>99%: 73.30 ms (59.21–90.99 ms)<br>C: 35.66 MiB (35.43–37.98 MiB)<br>S: 25.35 MiB (24.98–25.75 MiB) | 50%: 12.54 ms (11.93–13.24 ms)<br>99%: 34.82 ms (33.37–44.75 ms)<br>C: 38.88 MiB (37.43–39.15 MiB)<br>S: 25.90 MiB (25.77–25.91 MiB) | 50%: 6.84 ms (6.51–8.46 ms)<br>99%: 157.42 ms (122.50–160.02 ms)<br>C: 10.76 MiB (10.69–10.82 MiB)<br>S: 9.13 MiB (9.13–9.25 MiB) | 50%: 9.80 ms (9.11–10.45 ms)<br>99%: 55.46 ms (52.74–66.84 ms)<br>C: 133.46 MiB (133.14–137.96 MiB)<br>S: 87.45 MiB (87.18–89.26 MiB) |
| 800 B | 64 connections | 0% | 50%: 1.16 ms (1.12–1.17 ms)<br>99%: 4.95 ms (4.14–10.78 ms)<br>C: 11.84 MiB (11.81–12.00 MiB)<br>S: 3.22 MiB (3.21–3.38 MiB) | 50%: 1.11 ms (1.09–1.17 ms)<br>99%: 3.81 ms (3.35–3.89 ms)<br>C: 14.66 MiB (14.55–14.79 MiB)<br>S: 7.14 MiB (6.98–7.64 MiB) | 50%: 1.12 ms (1.09–1.17 ms)<br>99%: 5.94 ms (3.28–8.83 ms)<br>C: 14.79 MiB (14.64–14.91 MiB)<br>S: 7.30 MiB (7.28–7.46 MiB) | 50%: 1.18 ms (1.17–1.22 ms)<br>99%: 23.48 ms (6.77–30.71 ms)<br>C: 7.02 MiB (6.96–7.11 MiB)<br>S: 3.13 MiB (3.13–3.13 MiB) | 50%: 3.11 ms (3.08–3.14 ms)<br>99%: 14.75 ms (12.52–15.22 ms)<br>C: 36.69 MiB (36.36–38.85 MiB)<br>S: 32.20 MiB (30.51–32.23 MiB) |
| 800 B | 1,024 connections | 0% | 50%: 19.53 ms (19.52–21.90 ms)<br>99%: 40.40 ms (37.69–41.09 ms)<br>C: 20.02 MiB (20.00–20.07 MiB)<br>S: 4.92 MiB (4.91–5.05 MiB) | 50%: 13.14 ms (13.02–15.01 ms)<br>99%: 121.29 ms (116.86–193.91 ms)<br>C: 50.09 MiB (49.30–50.50 MiB)<br>S: 34.84 MiB (34.50–34.88 MiB) | 50%: 11.04 ms (10.65–18.39 ms)<br>99%: 184.17 ms (116.17–190.44 ms)<br>C: 51.06 MiB (50.85–52.21 MiB)<br>S: 34.95 MiB (34.54–36.98 MiB) | 50%: 17.84 ms (17.11–19.91 ms)<br>99%: 116.16 ms (113.56–120.76 ms)<br>C: 22.19 MiB (22.07–22.19 MiB)<br>S: 20.25 MiB (20.13–20.88 MiB) | 50%: 70.44 ms (69.77–72.96 ms)<br>99%: 124.10 ms (113.07–140.60 ms)<br>C: 227.46 MiB (226.21–237.71 MiB)<br>S: 156.83 MiB (151.87–162.71 MiB) |
| 800 B | 64 connections | 1% | 50%: 0.73 ms (0.68–0.79 ms)<br>99%: 5.66 ms (4.05–8.74 ms)<br>C: 11.75 MiB (11.65–12.10 MiB)<br>S: 3.38 MiB (3.38–3.39 MiB) | 50%: 1.07 ms (1.06–1.10 ms)<br>99%: 5.32 ms (5.02–7.29 ms)<br>C: 14.86 MiB (14.77–15.00 MiB)<br>S: 7.37 MiB (7.28–7.39 MiB) | 50%: 1.06 ms (1.04–1.10 ms)<br>99%: 5.98 ms (5.66–6.38 ms)<br>C: 15.10 MiB (15.08–15.12 MiB)<br>S: 7.72 MiB (7.51–7.82 MiB) | 50%: 1.16 ms (1.12–1.27 ms)<br>99%: 8.17 ms (7.62–15.36 ms)<br>C: 6.89 MiB (6.77–6.95 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.06 ms (3.02–3.06 ms)<br>99%: 13.40 ms (12.61–13.63 ms)<br>C: 37.86 MiB (36.83–38.06 MiB)<br>S: 29.83 MiB (29.78–34.01 MiB) |
| 800 B | 1,024 connections | 1% | 50%: 18.53 ms (18.27–18.59 ms)<br>99%: 46.00 ms (45.15–51.12 ms)<br>C: 20.23 MiB (20.10–20.32 MiB)<br>S: 4.86 MiB (4.73–4.91 MiB) | 50%: 15.22 ms (14.46–17.90 ms)<br>99%: 120.08 ms (68.63–121.71 ms)<br>C: 49.66 MiB (48.44–50.02 MiB)<br>S: 35.99 MiB (35.46–37.51 MiB) | 50%: 16.70 ms (15.52–18.56 ms)<br>99%: 116.65 ms (98.59–147.61 ms)<br>C: 51.54 MiB (50.38–52.66 MiB)<br>S: 36.85 MiB (36.84–37.36 MiB) | 50%: 18.00 ms (16.27–19.79 ms)<br>99%: 124.25 ms (120.94–127.48 ms)<br>C: 22.19 MiB (22.19–22.32 MiB)<br>S: 19.88 MiB (19.38–20.38 MiB) | 50%: 67.98 ms (67.98–69.97 ms)<br>99%: 174.05 ms (172.49–319.53 ms)<br>C: 243.64 MiB (243.08–245.33 MiB)<br>S: 198.14 MiB (181.47–228.02 MiB) |

### Reading the results

- With 64 B messages and 1,024 connections without injected loss, ordinary
  rust-raknet delivers 48.66 MiB/s and batch APIs
  deliver 142.12 MiB/s. TCP delivers 55.28 MiB/s,
  C KCP 42.03 MiB/s and quic-go 109.34 MiB/s.
- For that same profile, loaded median / 99th-percentile RTT is
  15.99 ms / 87.61 ms
  with ordinary sends and 4.76 ms / 19.49 ms
  with batch APIs. All five loaded-latency columns use the same per-message
  measurement.
- Check tails separately from throughput. At 800 B, 1,024 connections and
  1% loss, 99th-percentile loaded RTT is 120.08 ms for ordinary
  sends and 116.65 ms for batch APIs.
- At 800 B and 2,048 connections without injected loss, server peak RSS is
  6.61 MiB for TCP, 69.42 MiB for ordinary rust-raknet,
  74.59 MiB for batch rust-raknet, 38.38 MiB for C KCP and
  380.00 MiB for quic-go. This includes the drivers and runtimes;
  it is not a heap-only comparison of protocol cores.

The concurrent batch client constructs owned payloads for pending messages,
while the ordinary client reuses a borrowed template. Results include these
buffer choices and receive draining; they are not a pure packet-packing ablation.
Batch APIs are workload-dependent, especially when packing has fallen back to
individual sends. Compare throughput, loaded latency and memory together.

Random netem loss and namespace-local UDP receive-buffer errors were recorded
for every run. Saturated UDP sockets can drop packets even with no injected
loss. Maximum receive-buffer drops in one throughput run: rust-raknet (`send`): 207,572 datagrams, rust-raknet (batch APIs): 327,373 datagrams, C KCP: 370,281 datagrams, quic-go: 0 datagrams.
These are part of the measured workload. Random loss and shared CPU scheduling
make the ranges relevant; a small median difference is not a universal ranking.

All 445 measurements completed with verified echoes: 120 single-connection
throughput runs, 25 sparse-latency runs, 180 concurrent-throughput runs and
120 loaded-latency runs. This batch replaces the previous tables; it is not a
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
