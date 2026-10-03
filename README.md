<div align="center">

<img src="assets/logo.png" alt="rust-raknet otter logo" width="180">

# rust-raknet

**English** | [简体中文](README.zh-CN.md)

**A high-performance implementation of the RakNet protocol in Rust.**

[![Build](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet)
[![Documentation](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
[![Wiki](https://img.shields.io/badge/Wiki-EN%20%2F%20中文-007C83?logo=github)](https://github.com/b23r0/rust-raknet/wiki)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2)](https://discord.gg/ZKtYMvDFN4)

[Features](#features) · [Wiki](https://github.com/b23r0/rust-raknet/wiki) · [Get started](#get-started) · [Bedrock proxy](#minecraft-bedrock) · [Benchmarks](#benchmarks) · [Contributing](#contributing)

</div>

`rust-raknet` provides reliable message delivery over UDP for Rust applications.
Built on Tokio, it implements RakNet handshakes, acknowledgements, retransmission,
ordering and fragmentation. Choose the delivery guarantees your application needs.

## Features

- **Five delivery modes.** Send best-effort updates, discard stale state, deliver reliable events, or keep messages ordered on independent channels.
- **Async client and server.** Tokio-based connect, listen, accept, send and receive APIs, with IPv4 and IPv6 address encoding.
- **Message boundaries.** Receive complete application messages rather than reconstructing them from a byte stream. Large `ReliableOrdered` messages are fragmented and reassembled automatically.
- **Loss recovery.** ACK/NACK ranges, selective retries and ACK-gap fast retransmission, with RTT-based retry timers and retransmission backoff.
- **Explicit batching.** Pack ready small `ReliableOrdered` messages into standard RakNet datagrams with `send_batch` or `send_bytes_batch`. There is no timer to wait for more messages.
- **Owned buffer forwarding.** `Bytes` send/receive APIs share immutable payload storage with send and retry queues, reducing copies in relays.
- **Bounded queues and backpressure.** Send accounting, receive reordering and fragment assembly have limits; sustained producers wait for capacity.
- **Optional send policy.** The `send-policy` feature exposes per-connection flight limits, reliable/unreliable queue budgets and flush budgets. It is opt-in; see the measured [performance trade-offs](#performance-trade-offs).
- **Server tuning.** Configure the nominal MTU, accept backlog and UDP receive buffer. Optional Linux socket sharding, receive batching and idle maintenance let you tune busy listeners.
- **Discovery and lifecycle.** Unconnected ping/pong, customizable MOTD, peer RakNet version lookup, flush and close APIs.
- **Runnable examples.** Echo, discovery, reverse proxy and benchmark programs live in one [`examples/`](examples) directory.
- **Rust implementation.** Rust 2024, MIT licensed, with Linux, Windows, macOS and BSD support. Platform-specific fast paths have portable fallbacks.

**Requirements:** Rust 1.85+ and Tokio 1.38+.

## Documentation

The **[Wiki](https://github.com/b23r0/rust-raknet/wiki)** covers setup, delivery modes,
configuration, protocol details and reproducible benchmarks. English is the default;
each guide links to its Chinese translation.

[Quick start](https://github.com/b23r0/rust-raknet/wiki/Quick-Start-EN) ·
[RakNet protocol reference](https://github.com/b23r0/rust-raknet/wiki/Protocol-Reference-EN) ·
[Send policy](https://github.com/b23r0/rust-raknet/wiki/Send-Policy-EN) ·
[Benchmark methodology](https://github.com/b23r0/rust-raknet/wiki/Benchmark-Methodology-EN)

See [docs.rs](https://docs.rs/rust-raknet/latest/rust_raknet/) for the published API.
The protocol reference documents this implementation's limits as well as its wire format;
it does not imply support for every feature of the original RakNet SDK.

## Get started

```toml
[dependencies]
rust-raknet = "1.1.0"
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

### Optional send policy

The benchmark tables below distinguish default builds from explicitly configured
`send-policy` builds.

Available since 1.1.0. Enable the `send-policy` feature:

```toml
rust-raknet = { version = "1.1.0", features = ["send-policy"] }
```

Default builds compile the original send queue and feedback path without the new
policy state or checks. With `send-policy` enabled, call `set_send_options` to activate
the policy for a connection. Until then, the original
shared queue and 64-frame window remain active; `send_options().await` returns
`None`. Applying options returns `Some(options)` on subsequent queries.

With this policy enabled, unreliable messages have a
separate pending queue and budget, so a full reliable window does not prevent
admission of a small realtime update. A mixed flush rotates between reliable
retries, reliable new frames and unreliable frames in byte-sized quanta.
Unconfigured connections retain the original send and batch paths. No message is held
back to collect a batch, and accepted unreliable messages are not replaced or
silently evicted by this scheduler.

Configure each connected client or accepted server socket before bulk sending:

```rust
use rust_raknet::SendOptions;

let options = SendOptions::default()
    .with_in_flight_limits(64, 256 * 1024)?
    .with_queue_budgets(256 * 1024, 64 * 1024)?
    .with_flush_budget(256 * 1024)?;
socket.set_send_options(options).await?;
```

Flight bytes count each encoded reliable frame, including fragments and its
frame-set header; packed frames count separately. Queue budgets include payload
and estimated frame metadata. They are not process-memory or network-bandwidth
limits. The reliable budget includes unacknowledged data. A larger reliable send, including a batch,
may exceed its soft queue budget, but newly admitted reliable traffic
must leave the configured unreliable reserve within the 64 MiB accounting cap.
Messages accepted before options are applied are retained even if a new limit
is smaller; they drain before more traffic is admitted.

Larger windows can improve throughput at high RTT, but can also increase queueing
and loss at a shared bottleneck. The scheduler does not provide adaptive
congestion control or a guaranteed bandwidth split. A bounded flush continues
remaining work through the existing connection maintenance task; it adds no
new batching timer. A very small custom flush budget can defer remaining
frames until another send, ACK or maintenance tick; keep the default unless
you intend to limit bursts. Use separate sender tasks when an application needs realtime
updates to proceed while another `send` is awaiting reliable capacity.

Loss feedback keeps the original fast retry and packing fallback behavior
in both configured and unconfigured connections.

#### Performance trade-offs

`send-policy` is a control over queue admission, reliable flight and sending
bursts; it is not a throughput preset. Default builds omit the policy code.
A feature-enabled socket without a setter retains the original queue strategy,
but is not the default binary; the configured benchmark columns also call the
setter on both endpoints.

The benchmark policy preset is **`SendOptions::default()`**, whose flight-byte
limit is **64 MiB**. The example above changes that limit to **256 KiB**; its
results cannot be inferred directly from the preset's benchmark column. Calling
`set_send_options(SendOptions::default())` activates the policy; it does not
restore the original unconfigured queue strategy.

| Control | Potential benefit | Cost to measure |
| --- | --- | --- |
| Flight frames / bytes | Limit outstanding reliable data and bottleneck queue pressure | Smaller windows can cap healthy high-RTT throughput; larger ones can deepen queues and loss |
| Separate queue budgets | Keep capacity for unreliable updates while reliable sends wait | Both classes still share the link; increased unreliable admission can reduce reliable bandwidth or increase network loss |
| Flush bytes | Bound each send burst and give other work a chance to run | Small budgets can defer queued frames until another send, ACK or maintenance tick |

Results from October 3, using the default policy preset; values below are
run medians. [The benchmark tables](#benchmarks) include ranges and memory.

| Reliable ordered workload | Without policy | Configured policy | Median change |
| --- | --- | --- | --- |
| One connection, 800 B, no injected loss, ordinary throughput | 185.57 MiB/s | 175.19 MiB/s | −5.6% |
| 1,024 connections, 800 B, no injected loss, ordinary throughput | 464.75 MiB/s | 486.67 MiB/s | +4.7% |
| 1,024 connections, 800 B, no injected loss, batch throughput | 481.91 MiB/s | 423.69 MiB/s | −12.1% |
| 1,024 connections, 64 B, no injected loss, ordinary loaded 99th-percentile RTT | 54.03 ms | 100.53 ms | +86.1% (slower) |

These three-repeat measurements show trade-offs, not statistical significance.
Policy accounting and scheduling can add work and change the timing of admitted
messages and bursts. This can affect throughput and slow-message latency even
without changing the wire protocol; the measurements do not isolate a single
cause for each difference.

Separate mixed-traffic tests used eight connections in separate processes,
sharing a link that dropped from 100 Mbps / 30 ms RTT to 2 Mbps / 800 ms RTT.
Candidate senders used the default policy preset. During congestion, unreliable
slowest-1% delivered-message latency improved from
1,894.73 ms to 743.49 ms, but reliable throughput fell from 0.072704 Mbps to
0.059392 Mbps. Across the whole run, unreliable delivery fell from 68.65% to
60.01%. Queue isolation does not guarantee delivery or remove network congestion.

For reliable-only traffic, start with the default build. Enable the policy when
you need configurable limits or mixed queue isolation, then measure your payloads,
connection count and network. Use separate sender tasks for mixed traffic, and
compare throughput, loaded latency, delivery rate and memory together. There is
no automatic congestion adaptation or universally optimal window.

See the [send policy Wiki](https://github.com/b23r0/rust-raknet/wiki/Send-Policy-EN)
for setting options on clients, accepted sockets and proxy legs, and for changing
limits on an existing connection.

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

These measurements compare this **rust-raknet implementation** with TCP, the official C KCP core and quic-go. Ordinary sends and batch APIs are measured with the default build and with an explicitly configured `send-policy`. They do not measure the RakNet protocol in general.

**Measured on October 3, 2026.** Intel Core i7-9700F (8 cores), Linux x86_64, kernel `7.0.11-76070011-generic`; Rust 1.98.1, Tokio 1.53.1 and Go 1.27.1; optimized builds. All processes ran in a task copy with private caches and a private loopback network namespace. Loopback MTU was 1,500 B, GSO/GRO was limited to one packet, netem's queue limit was 100,000 packets, and processes used `nice 10`. Host network settings were unchanged.

### Implementations and API modes

| Column | GitHub source | Tested version / configuration |
| --- | --- | --- |
| TCP | [TCP echo driver](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark), [Tokio](https://github.com/tokio-rs/tokio) | Driver 0.1.0; Tokio 1.53.1; Linux TCP stack 7.0.11-76070011-generic |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 1.0.0 plus the send-policy changes in this commit; base revision `7b72b25`; default build |
| rust-raknet (batch APIs) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | The same default library build; explicit batch APIs |
| rust-raknet (`send` + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | Same source; feature enabled; both endpoints call `set_send_options(SendOptions::default())` |
| rust-raknet (batch APIs + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | Same configured build; explicit batch APIs |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | Pinned commit `b1a7a21`; upstream C core unchanged |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

- **Ordinary sends**: both endpoints use `send` / `recv`, without batch API calls.
- **Batch APIs**: the single-connection client uses `send_batch`; the concurrent client uses `send_bytes_batch`. Echo servers use `recv_bytes_batch` and `send_bytes_batch`. Each call submits at most 16 ready messages; neither endpoint waits to fill a batch.
- **Default build**: `send-policy` is not compiled. Policy columns enable the feature and call `set_send_options(SendOptions::default())` on both endpoints.

All four rust-raknet columns use `ReliableOrdered` and a 64-frame reliable flight limit. Policy columns also use a 64 MiB encoded flight-byte limit, a 256 KiB reliable soft queue budget, a 64 KiB unreliable reserve and a 256 KiB flush budget. Optional UDP receive batching and idle maintenance are disabled. Message coalescing is separate from UDP syscall batching.

A UDP datagram packs at most eight non-fragmented messages, within the MTU. Two 800 B messages do not fit together; 4,096 B messages are fragmented. A matched NACK or reliable retransmission timeout disables packing for the rest of that connection, including feedback during warmup. **Calling a batch API does not mean every run actually merges messages.**

TCP uses `TCP_NODELAY`, four-byte little-endian record lengths, complete-record writes and reused buffers. C KCP uses message mode, send/receive windows of 64/64 for single-connection runs and 64/128 for concurrent runs, `nodelay(1, 10, 2, 1)`, immediate writes/ACKs, and no FEC or encryption. QUIC uses one bidirectional reliable ordered stream per connection; TLS encryption, congestion control and flow control remain enabled. These implementations do not provide identical features.

Throughput counts verified echoed application payload **per direction**; higher is better. Lower RTT is better. 1 MiB = 1,048,576 B. Throughput and loaded-latency profiles run three times; sparse latency runs five times. Order rotates across all seven columns, with fresh servers and connections. Random loss affects data and ACKs in both directions.

Values are medians across repetitions; parentheses show the min–max range. Each value includes its unit. **C = client, S = server.** Ranges and peak RSS are included in each measured table. RTT percentiles are computed within each run, then the median of each run-level percentile is reported.

Linux `wait4` records whole-process peak RSS across startup, setup, warmup, load and teardown. It includes runtimes, allocators, application buffers and latency samples, but excludes kernel socket buffers. RSS is neither total network memory nor a per-connection footprint; client and server peaks need not occur together.

### Single connection

Clients keep at most 64 application messages awaiting echoes. Each run has 100 warmups and 300 sequential RTT samples before the measured burst. Rust and Go use four workers; C KCP uses one event loop per process. Throughput runs are not pinned to CPUs, so these are not equal-CPU-efficiency measurements.

| Network profile | Payload / burst count | TCP<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send` + `send-policy`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs + `send-policy`)<br>Throughput (range)<br>RSS: C / S | C KCP<br>Throughput (range)<br>RSS: C / S | quic-go<br>Throughput (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| No injected loss | 800 B / 200,000 messages | 154.44 MiB/s (152.93–160.66 MiB/s)<br>C: 3.53 MiB (3.52–3.66 MiB)<br>S: 3.43 MiB (3.41–3.50 MiB) | 185.57 MiB/s (176.25–186.72 MiB/s)<br>C: 3.92 MiB (3.79–3.93 MiB)<br>S: 3.74 MiB (3.67–3.75 MiB) | 169.67 MiB/s (169.62–175.89 MiB/s)<br>C: 3.86 MiB (3.85–3.90 MiB)<br>S: 3.76 MiB (3.74–4.12 MiB) | 175.19 MiB/s (158.46–176.84 MiB/s)<br>C: 3.93 MiB (3.90–4.02 MiB)<br>S: 3.78 MiB (3.71–3.87 MiB) | 172.00 MiB/s (135.85–193.53 MiB/s)<br>C: 3.92 MiB (3.78–4.03 MiB)<br>S: 4.04 MiB (3.81–4.07 MiB) | 153.60 MiB/s (145.14–157.41 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 112.61 MiB/s (109.10–117.96 MiB/s)<br>C: 14.11 MiB (14.08–14.11 MiB)<br>S: 13.86 MiB (13.78–14.08 MiB) |
| 1% loss | 800 B / 50,000 messages | 24.07 MiB/s (22.39–75.22 MiB/s)<br>C: 3.32 MiB (3.31–3.54 MiB)<br>S: 3.43 MiB (3.20–3.45 MiB) | 153.70 MiB/s (153.41–172.75 MiB/s)<br>C: 3.92 MiB (3.91–4.00 MiB)<br>S: 3.91 MiB (3.90–3.95 MiB) | 157.34 MiB/s (151.22–178.39 MiB/s)<br>C: 3.79 MiB (3.77–4.11 MiB)<br>S: 3.93 MiB (3.86–4.04 MiB) | 151.29 MiB/s (145.01–178.60 MiB/s)<br>C: 3.73 MiB (3.66–3.79 MiB)<br>S: 3.78 MiB (3.73–3.96 MiB) | 153.44 MiB/s (148.61–166.67 MiB/s)<br>C: 3.86 MiB (3.72–3.97 MiB)<br>S: 3.72 MiB (3.62–4.04 MiB) | 130.51 MiB/s (130.29–134.65 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) | 65.26 MiB/s (61.60–89.68 MiB/s)<br>C: 13.81 MiB (13.30–13.92 MiB)<br>S: 14.07 MiB (13.44–14.13 MiB) |
| 5% loss | 800 B / 50,000 messages | 1.03 MiB/s (0.96–1.14 MiB/s)<br>C: 3.60 MiB (3.54–3.61 MiB)<br>S: 3.43 MiB (3.41–3.55 MiB) | 140.90 MiB/s (134.09–141.29 MiB/s)<br>C: 3.86 MiB (3.71–3.86 MiB)<br>S: 3.90 MiB (3.63–3.91 MiB) | 155.22 MiB/s (137.11–166.69 MiB/s)<br>C: 3.77 MiB (3.75–3.91 MiB)<br>S: 3.78 MiB (3.74–3.99 MiB) | 140.20 MiB/s (138.98–141.27 MiB/s)<br>C: 3.71 MiB (3.51–3.75 MiB)<br>S: 3.72 MiB (3.58–3.88 MiB) | 141.24 MiB/s (107.59–145.54 MiB/s)<br>C: 3.84 MiB (3.82–3.88 MiB)<br>S: 3.73 MiB (3.70–3.91 MiB) | 104.70 MiB/s (100.74–114.25 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 9.96 MiB/s (8.77–10.80 MiB/s)<br>C: 14.06 MiB (13.82–14.19 MiB)<br>S: 13.76 MiB (13.66–14.36 MiB) |
| No injected loss | 64 B / 300,000 messages | 14.01 MiB/s (13.91–14.17 MiB/s)<br>C: 3.48 MiB (3.31–3.68 MiB)<br>S: 3.41 MiB (3.33–3.53 MiB) | 16.40 MiB/s (15.99–16.64 MiB/s)<br>C: 3.67 MiB (3.60–3.70 MiB)<br>S: 3.75 MiB (3.73–3.79 MiB) | 48.44 MiB/s (48.35–51.05 MiB/s)<br>C: 3.75 MiB (3.65–3.81 MiB)<br>S: 3.64 MiB (3.59–3.79 MiB) | 16.59 MiB/s (16.15–16.63 MiB/s)<br>C: 3.66 MiB (3.57–3.68 MiB)<br>S: 3.80 MiB (3.53–3.98 MiB) | 49.40 MiB/s (47.83–51.00 MiB/s)<br>C: 3.71 MiB (3.57–3.89 MiB)<br>S: 3.72 MiB (3.55–3.73 MiB) | 12.33 MiB/s (12.03–12.48 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 11.16 MiB/s (10.08–19.35 MiB/s)<br>C: 13.57 MiB (13.57–13.57 MiB)<br>S: 13.19 MiB (12.94–13.69 MiB) |
| 1% loss | 64 B / 100,000 messages | 2.49 MiB/s (2.02–2.54 MiB/s)<br>C: 3.71 MiB (3.70–3.75 MiB)<br>S: 3.40 MiB (3.38–3.54 MiB) | 14.40 MiB/s (14.28–14.79 MiB/s)<br>C: 3.73 MiB (3.67–3.77 MiB)<br>S: 3.55 MiB (3.49–3.75 MiB) | 16.06 MiB/s (15.09–16.07 MiB/s)<br>C: 3.68 MiB (3.62–3.79 MiB)<br>S: 3.71 MiB (3.56–3.76 MiB) | 16.38 MiB/s (14.46–16.54 MiB/s)<br>C: 3.81 MiB (3.68–3.86 MiB)<br>S: 3.72 MiB (3.66–3.74 MiB) | 14.63 MiB/s (14.40–14.98 MiB/s)<br>C: 3.75 MiB (3.57–3.88 MiB)<br>S: 3.65 MiB (3.62–3.89 MiB) | 11.25 MiB/s (10.75–11.48 MiB/s)<br>C: 1.88 MiB (1.88–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 23.94 MiB/s (11.01–26.12 MiB/s)<br>C: 12.88 MiB (12.44–13.07 MiB)<br>S: 12.07 MiB (11.69–12.07 MiB) |
| 5% loss | 64 B / 100,000 messages | 0.13 MiB/s (0.13–0.15 MiB/s)<br>C: 3.55 MiB (3.52–3.70 MiB)<br>S: 3.38 MiB (3.20–3.44 MiB) | 13.73 MiB/s (13.53–13.91 MiB/s)<br>C: 3.70 MiB (3.63–3.88 MiB)<br>S: 3.59 MiB (3.47–3.67 MiB) | 13.48 MiB/s (10.60–13.62 MiB/s)<br>C: 3.71 MiB (3.67–3.74 MiB)<br>S: 3.78 MiB (3.68–3.82 MiB) | 13.42 MiB/s (13.42–13.51 MiB/s)<br>C: 3.65 MiB (3.64–3.69 MiB)<br>S: 3.63 MiB (3.61–3.95 MiB) | 13.45 MiB/s (13.11–13.77 MiB/s)<br>C: 3.66 MiB (3.60–3.80 MiB)<br>S: 3.79 MiB (3.77–3.80 MiB) | 10.01 MiB/s (9.91–10.64 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 3.59 MiB/s (2.81–3.69 MiB/s)<br>C: 12.76 MiB (12.69–13.07 MiB)<br>S: 11.94 MiB (11.94–12.00 MiB) |
| No injected loss | 4,096 B / 50,000 messages | 346.57 MiB/s (344.74–351.15 MiB/s)<br>C: 3.58 MiB (3.55–3.66 MiB)<br>S: 3.40 MiB (3.39–3.46 MiB) | 303.25 MiB/s (279.28–312.47 MiB/s)<br>C: 4.77 MiB (4.36–4.98 MiB)<br>S: 5.10 MiB (4.91–5.29 MiB) | 309.56 MiB/s (306.58–313.50 MiB/s)<br>C: 4.90 MiB (4.45–5.11 MiB)<br>S: 4.80 MiB (4.77–5.01 MiB) | 310.12 MiB/s (307.68–312.53 MiB/s)<br>C: 4.49 MiB (4.43–5.10 MiB)<br>S: 4.99 MiB (4.83–5.08 MiB) | 313.04 MiB/s (310.98–313.37 MiB/s)<br>C: 4.70 MiB (4.61–4.77 MiB)<br>S: 4.95 MiB (4.79–4.95 MiB) | 253.51 MiB/s (249.69–257.45 MiB/s)<br>C: 1.86 MiB (1.86–1.87 MiB)<br>S: 1.87 MiB (1.75–1.87 MiB) | 220.97 MiB/s (80.24–226.14 MiB/s)<br>C: 14.09 MiB (13.90–14.12 MiB)<br>S: 14.00 MiB (13.65–14.14 MiB) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.08 MiB/s (1.04–1.13 MiB/s)<br>C: 3.66 MiB (3.51–3.75 MiB)<br>S: 3.43 MiB (3.41–3.56 MiB) | 2.95 MiB/s (2.80–3.09 MiB/s)<br>C: 3.76 MiB (3.53–3.78 MiB)<br>S: 3.72 MiB (3.71–3.86 MiB) | 3.01 MiB/s (2.90–3.03 MiB/s)<br>C: 3.86 MiB (3.69–3.98 MiB)<br>S: 3.85 MiB (3.62–3.88 MiB) | 3.09 MiB/s (2.89–3.39 MiB/s)<br>C: 3.76 MiB (3.69–3.85 MiB)<br>S: 3.76 MiB (3.60–3.90 MiB) | 2.91 MiB/s (2.88–3.02 MiB/s)<br>C: 3.81 MiB (3.62–3.86 MiB)<br>S: 3.75 MiB (3.55–3.75 MiB) | 2.88 MiB/s (2.86–3.24 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 1.78 MiB/s (1.62–1.81 MiB/s)<br>C: 11.57 MiB (11.32–11.63 MiB)<br>S: 11.69 MiB (11.57–11.69 MiB) |

Single-connection rust-raknet uses a 1,428 B nominal MTU, including 28 B of IPv4/UDP overhead; C KCP and quic-go use a 1,400 B UDP packet budget. The 4,096 B profile triggers fragmentation in rust-raknet and C KCP.

### Sparse request/echo latency

800 B messages, no injected loss; five runs, each with 1,000 warmups and 10,000 sequential samples. Client and server are pinned to different CPUs; each Rust/Go process has four workers, while C KCP uses one event loop. Each request waits for its echo. The batch API receives only one message per call; nothing is coalesced here.

| Implementation / API | Median RTT (range) | 95th-percentile RTT (range) | 99th-percentile RTT (range) | Peak RSS: C / S (range) |
| --- | --- | --- | --- | --- |
| TCP | 12.80 µs (12.70–13.00 µs) | 19.50 µs (18.90–21.20 µs) | 40.60 µs (38.60–42.40 µs) | C: 3.88 MiB (3.88–4.02 MiB)<br>S: 3.56 MiB (3.47–3.60 MiB) |
| rust-raknet (`send`) | 17.40 µs (17.30–18.20 µs) | 30.90 µs (25.90–31.30 µs) | 49.50 µs (48.50–51.30 µs) | C: 4.25 MiB (4.15–4.32 MiB)<br>S: 4.07 MiB (3.95–4.39 MiB) |
| rust-raknet (batch APIs) | 17.80 µs (17.50–18.70 µs) | 31.00 µs (24.10–32.50 µs) | 50.10 µs (49.00–58.60 µs) | C: 4.22 MiB (4.18–4.35 MiB)<br>S: 4.07 MiB (3.96–4.24 MiB) |
| rust-raknet (`send` + `send-policy`) | 17.90 µs (17.40–18.00 µs) | 30.30 µs (26.00–32.30 µs) | 49.40 µs (48.10–53.30 µs) | C: 4.26 MiB (4.20–4.38 MiB)<br>S: 4.10 MiB (4.00–4.21 MiB) |
| rust-raknet (batch APIs + `send-policy`) | 17.80 µs (17.40–18.90 µs) | 31.50 µs (27.40–32.20 µs) | 50.50 µs (46.60–53.20 µs) | C: 4.21 MiB (4.04–4.25 MiB)<br>S: 4.12 MiB (4.04–4.20 MiB) |
| C KCP | 10.30 µs (10.10–10.40 µs) | 15.80 µs (13.10–16.10 µs) | 34.80 µs (34.30–35.00 µs) | C: 1.88 MiB (1.87–2.00 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) |
| quic-go | 62.50 µs (62.30–62.80 µs) | 102.10 µs (99.90–103.10 µs) | 146.10 µs (143.90–152.10 µs) | C: 13.25 MiB (12.94–13.44 MiB)<br>S: 10.94 MiB (10.94–11.06 MiB) |

These measure the sparse API path, not burst delivery latency. Concurrent throughput's pre-burst RTT samples are also not used as loaded latency below.

### Concurrent throughput

Each run verifies 1,048,576 ordered burst echoes with a 16-message application window per connection. Connection IDs, message IDs, complete payloads and ordering are checked. Twenty sequential samples per connection precede a common start barrier; setup and warmup are excluded from the throughput timer. Connections stay open until every burst finishes.

All seven columns use four workers and four server listeners/receive sockets with `SO_REUSEPORT`. Servers are pinned to CPUs 0–3 and clients to CPUs 4–7. Affinity does not reserve CPUs exclusively.

| Payload | Connections | Injected loss | TCP<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send` + `send-policy`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs + `send-policy`)<br>Throughput (range)<br>RSS: C / S | C KCP<br>Throughput (range)<br>RSS: C / S | quic-go<br>Throughput (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 connections | 0% | 647.10 MiB/s (618.06–652.91 MiB/s)<br>C: 3.89 MiB (3.86–3.96 MiB)<br>S: 3.32 MiB (3.24–3.55 MiB) | 572.91 MiB/s (539.19–595.28 MiB/s)<br>C: 7.13 MiB (6.96–7.16 MiB)<br>S: 7.58 MiB (7.43–7.89 MiB) | 599.30 MiB/s (538.39–601.84 MiB/s)<br>C: 7.25 MiB (7.12–7.29 MiB)<br>S: 7.89 MiB (7.77–7.98 MiB) | 544.68 MiB/s (522.42–572.92 MiB/s)<br>C: 6.98 MiB (6.86–7.32 MiB)<br>S: 7.55 MiB (7.45–7.77 MiB) | 572.00 MiB/s (556.00–572.14 MiB/s)<br>C: 7.38 MiB (7.25–7.45 MiB)<br>S: 7.82 MiB (7.62–7.91 MiB) | 386.44 MiB/s (375.77–405.71 MiB/s)<br>C: 2.88 MiB (2.88–3.25 MiB)<br>S: 3.13 MiB (3.13–3.38 MiB) | 226.60 MiB/s (217.63–228.88 MiB/s)<br>C: 30.00 MiB (29.82–30.17 MiB)<br>S: 34.61 MiB (32.57–35.63 MiB) |
| 800 B | 256 connections | 0% | 590.00 MiB/s (504.74–591.30 MiB/s)<br>C: 5.76 MiB (5.68–5.77 MiB)<br>S: 3.62 MiB (3.45–3.76 MiB) | 567.45 MiB/s (541.52–573.54 MiB/s)<br>C: 15.67 MiB (15.66–16.02 MiB)<br>S: 14.22 MiB (14.05–14.38 MiB) | 544.23 MiB/s (534.40–562.60 MiB/s)<br>C: 16.43 MiB (16.24–16.64 MiB)<br>S: 14.94 MiB (14.79–15.10 MiB) | 569.72 MiB/s (507.60–570.51 MiB/s)<br>C: 15.79 MiB (15.64–16.14 MiB)<br>S: 14.40 MiB (14.21–14.53 MiB) | 524.99 MiB/s (513.03–527.06 MiB/s)<br>C: 16.95 MiB (16.61–16.99 MiB)<br>S: 14.85 MiB (14.07–15.06 MiB) | 396.53 MiB/s (335.64–405.05 MiB/s)<br>C: 6.50 MiB (6.38–6.50 MiB)<br>S: 6.88 MiB (6.75–7.00 MiB) | 189.10 MiB/s (188.33–189.23 MiB/s)<br>C: 77.04 MiB (76.49–79.95 MiB)<br>S: 86.39 MiB (75.70–87.51 MiB) |
| 800 B | 1,024 connections | 0% | 570.54 MiB/s (569.63–581.21 MiB/s)<br>C: 11.45 MiB (10.53–12.27 MiB)<br>S: 4.81 MiB (4.66–4.99 MiB) | 464.75 MiB/s (461.92–554.91 MiB/s)<br>C: 47.22 MiB (47.02–49.01 MiB)<br>S: 39.73 MiB (39.58–43.51 MiB) | 481.91 MiB/s (452.22–505.51 MiB/s)<br>C: 48.61 MiB (47.86–49.06 MiB)<br>S: 42.52 MiB (41.35–43.19 MiB) | 486.67 MiB/s (463.14–486.71 MiB/s)<br>C: 46.94 MiB (46.79–49.04 MiB)<br>S: 41.18 MiB (39.42–41.68 MiB) | 423.69 MiB/s (361.22–521.99 MiB/s)<br>C: 48.64 MiB (48.37–49.91 MiB)<br>S: 42.30 MiB (39.68–44.45 MiB) | 369.82 MiB/s (343.27–383.97 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 20.63 MiB (20.63–20.88 MiB) | 161.86 MiB/s (154.55–168.66 MiB/s)<br>C: 242.18 MiB (240.76–247.57 MiB)<br>S: 242.51 MiB (198.42–273.20 MiB) |
| 800 B | 2,048 connections | 0% | 569.08 MiB/s (553.16–570.47 MiB/s)<br>C: 18.93 MiB (16.64–20.12 MiB)<br>S: 6.61 MiB (6.46–6.67 MiB) | 395.73 MiB/s (370.16–429.25 MiB/s)<br>C: 87.11 MiB (86.32–88.48 MiB)<br>S: 70.56 MiB (69.97–71.50 MiB) | 374.79 MiB/s (373.14–377.43 MiB/s)<br>C: 89.93 MiB (87.41–91.23 MiB)<br>S: 73.80 MiB (71.18–74.89 MiB) | 418.58 MiB/s (413.37–425.69 MiB/s)<br>C: 86.88 MiB (86.30–87.59 MiB)<br>S: 71.92 MiB (68.51–73.05 MiB) | 384.77 MiB/s (382.97–399.33 MiB/s)<br>C: 89.42 MiB (88.61–89.62 MiB)<br>S: 73.54 MiB (72.30–74.41 MiB) | 325.00 MiB/s (314.37–356.58 MiB/s)<br>C: 38.88 MiB (38.75–38.88 MiB)<br>S: 38.88 MiB (38.63–38.88 MiB) | 117.58 MiB/s (117.50–141.56 MiB/s)<br>C: 473.72 MiB (469.05–529.84 MiB)<br>S: 416.27 MiB (387.32–471.65 MiB) |
| 800 B | 64 connections | 1% | 453.95 MiB/s (415.34–470.16 MiB/s)<br>C: 4.07 MiB (4.02–4.23 MiB)<br>S: 3.44 MiB (3.44–3.50 MiB) | 515.14 MiB/s (488.61–521.14 MiB/s)<br>C: 7.24 MiB (7.10–7.75 MiB)<br>S: 7.60 MiB (7.52–8.20 MiB) | 536.13 MiB/s (523.38–558.63 MiB/s)<br>C: 7.49 MiB (7.32–7.64 MiB)<br>S: 7.76 MiB (7.57–7.87 MiB) | 523.12 MiB/s (466.22–540.41 MiB/s)<br>C: 7.46 MiB (7.25–7.47 MiB)<br>S: 7.63 MiB (7.54–8.09 MiB) | 502.00 MiB/s (457.08–542.14 MiB/s)<br>C: 7.61 MiB (7.39–7.66 MiB)<br>S: 7.87 MiB (7.76–8.07 MiB) | 369.95 MiB/s (360.65–384.65 MiB/s)<br>C: 2.88 MiB (2.75–3.00 MiB)<br>S: 3.25 MiB (3.25–3.25 MiB) | 218.94 MiB/s (203.58–221.61 MiB/s)<br>C: 28.92 MiB (28.11–29.08 MiB)<br>S: 31.34 MiB (31.06–37.63 MiB) |
| 800 B | 256 connections | 1% | 524.45 MiB/s (440.81–562.14 MiB/s)<br>C: 5.64 MiB (5.39–5.69 MiB)<br>S: 3.62 MiB (3.52–3.73 MiB) | 481.38 MiB/s (439.64–487.96 MiB/s)<br>C: 16.76 MiB (15.92–17.03 MiB)<br>S: 15.76 MiB (15.14–15.84 MiB) | 482.40 MiB/s (470.57–499.85 MiB/s)<br>C: 17.21 MiB (16.81–17.70 MiB)<br>S: 15.55 MiB (15.06–16.17 MiB) | 506.93 MiB/s (455.77–508.54 MiB/s)<br>C: 16.34 MiB (16.01–17.27 MiB)<br>S: 15.27 MiB (14.77–15.50 MiB) | 495.02 MiB/s (451.15–513.80 MiB/s)<br>C: 17.05 MiB (16.89–17.21 MiB)<br>S: 15.91 MiB (15.67–15.95 MiB) | 397.44 MiB/s (391.17–423.06 MiB/s)<br>C: 6.38 MiB (6.38–6.50 MiB)<br>S: 6.75 MiB (6.75–6.88 MiB) | 174.82 MiB/s (174.44–181.90 MiB/s)<br>C: 75.01 MiB (73.70–76.02 MiB)<br>S: 92.40 MiB (77.64–95.14 MiB) |
| 800 B | 1,024 connections | 1% | 477.11 MiB/s (421.92–496.74 MiB/s)<br>C: 12.33 MiB (12.24–12.44 MiB)<br>S: 4.82 MiB (4.68–4.95 MiB) | 450.82 MiB/s (436.15–481.93 MiB/s)<br>C: 48.47 MiB (48.41–48.57 MiB)<br>S: 41.77 MiB (41.41–42.66 MiB) | 426.79 MiB/s (424.29–444.82 MiB/s)<br>C: 50.86 MiB (49.43–52.42 MiB)<br>S: 43.78 MiB (42.70–43.86 MiB) | 399.41 MiB/s (396.93–439.76 MiB/s)<br>C: 49.58 MiB (46.70–51.19 MiB)<br>S: 42.44 MiB (42.13–43.79 MiB) | 435.19 MiB/s (425.02–444.45 MiB/s)<br>C: 50.48 MiB (50.29–51.38 MiB)<br>S: 43.03 MiB (42.79–44.09 MiB) | 360.22 MiB/s (347.16–361.17 MiB/s)<br>C: 20.25 MiB (20.25–20.25 MiB)<br>S: 19.88 MiB (19.50–20.50 MiB) | 98.72 MiB/s (94.11–126.29 MiB/s)<br>C: 304.08 MiB (256.50–333.60 MiB)<br>S: 203.75 MiB (202.14–270.56 MiB) |
| 800 B | 2,048 connections | 1% | 433.33 MiB/s (319.12–468.32 MiB/s)<br>C: 20.67 MiB (19.20–21.36 MiB)<br>S: 6.61 MiB (6.52–6.62 MiB) | 370.29 MiB/s (343.56–370.62 MiB/s)<br>C: 87.20 MiB (87.10–89.61 MiB)<br>S: 73.31 MiB (71.26–75.51 MiB) | 365.95 MiB/s (337.58–378.87 MiB/s)<br>C: 92.91 MiB (92.22–93.15 MiB)<br>S: 76.68 MiB (75.34–80.20 MiB) | 352.13 MiB/s (342.57–364.23 MiB/s)<br>C: 89.61 MiB (88.52–91.56 MiB)<br>S: 75.65 MiB (73.05–78.55 MiB) | 361.54 MiB/s (333.15–374.76 MiB/s)<br>C: 93.25 MiB (91.78–93.64 MiB)<br>S: 77.25 MiB (76.32–77.80 MiB) | 310.78 MiB/s (294.50–331.89 MiB/s)<br>C: 38.88 MiB (38.88–38.88 MiB)<br>S: 37.25 MiB (36.88–37.38 MiB) | 36.77 MiB/s (31.45–54.96 MiB/s)<br>C: 744.62 MiB (708.05–756.38 MiB)<br>S: 405.50 MiB (393.24–422.23 MiB) |
| 64 B | 64 connections | 0% | 66.86 MiB/s (62.89–67.40 MiB/s)<br>C: 3.95 MiB (3.75–4.17 MiB)<br>S: 3.30 MiB (3.25–3.39 MiB) | 48.08 MiB/s (45.28–48.43 MiB/s)<br>C: 5.73 MiB (5.68–5.89 MiB)<br>S: 5.86 MiB (5.81–6.00 MiB) | 106.70 MiB/s (105.15–121.63 MiB/s)<br>C: 5.75 MiB (5.69–5.97 MiB)<br>S: 6.20 MiB (6.16–6.26 MiB) | 47.19 MiB/s (46.29–47.47 MiB/s)<br>C: 5.66 MiB (5.62–5.83 MiB)<br>S: 5.88 MiB (5.87–6.05 MiB) | 112.17 MiB/s (110.42–114.05 MiB/s)<br>C: 5.77 MiB (5.59–5.98 MiB)<br>S: 6.20 MiB (5.95–6.24 MiB) | 32.94 MiB/s (31.28–35.36 MiB/s)<br>C: 2.38 MiB (2.25–2.38 MiB)<br>S: 2.50 MiB (2.38–2.50 MiB) | 138.04 MiB/s (130.66–138.30 MiB/s)<br>C: 21.69 MiB (21.46–22.21 MiB)<br>S: 19.14 MiB (18.86–19.57 MiB) |
| 64 B | 1,024 connections | 0% | 54.18 MiB/s (53.17–54.75 MiB/s)<br>C: 9.54 MiB (9.14–9.59 MiB)<br>S: 4.04 MiB (3.93–4.27 MiB) | 46.73 MiB/s (43.62–47.39 MiB/s)<br>C: 32.80 MiB (32.34–33.24 MiB)<br>S: 27.71 MiB (27.59–29.36 MiB) | 142.95 MiB/s (132.99–143.79 MiB/s)<br>C: 31.85 MiB (31.79–32.16 MiB)<br>S: 25.89 MiB (25.87–26.07 MiB) | 48.69 MiB/s (45.51–48.92 MiB/s)<br>C: 32.62 MiB (32.01–32.73 MiB)<br>S: 26.83 MiB (26.44–27.57 MiB) | 144.11 MiB/s (141.29–147.03 MiB/s)<br>C: 32.29 MiB (32.00–32.37 MiB)<br>S: 26.27 MiB (26.03–26.36 MiB) | 44.69 MiB/s (44.14–44.78 MiB/s)<br>C: 8.63 MiB (8.63–8.88 MiB)<br>S: 9.38 MiB (9.38–9.38 MiB) | 107.37 MiB/s (103.80–107.93 MiB/s)<br>C: 138.57 MiB (133.51–140.70 MiB)<br>S: 94.24 MiB (92.34–97.74 MiB) |
| 64 B | 64 connections | 1% | 43.37 MiB/s (38.98–45.56 MiB/s)<br>C: 3.85 MiB (3.72–3.93 MiB)<br>S: 3.36 MiB (3.25–3.37 MiB) | 45.08 MiB/s (43.00–46.26 MiB/s)<br>C: 5.82 MiB (5.81–5.96 MiB)<br>S: 6.12 MiB (6.04–6.16 MiB) | 48.32 MiB/s (45.90–49.52 MiB/s)<br>C: 6.00 MiB (5.94–6.21 MiB)<br>S: 6.39 MiB (6.39–6.43 MiB) | 45.22 MiB/s (44.37–46.15 MiB/s)<br>C: 5.84 MiB (5.77–5.89 MiB)<br>S: 6.05 MiB (6.04–6.29 MiB) | 46.12 MiB/s (44.19–49.77 MiB/s)<br>C: 5.84 MiB (5.83–5.84 MiB)<br>S: 6.39 MiB (6.39–6.49 MiB) | 33.61 MiB/s (32.28–34.08 MiB/s)<br>C: 2.38 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.25–2.38 MiB) | 56.68 MiB/s (55.71–58.67 MiB/s)<br>C: 21.63 MiB (21.57–22.01 MiB)<br>S: 18.76 MiB (18.54–19.51 MiB) |
| 64 B | 1,024 connections | 1% | 49.42 MiB/s (44.09–49.50 MiB/s)<br>C: 9.49 MiB (9.04–9.56 MiB)<br>S: 4.19 MiB (4.18–4.20 MiB) | 42.22 MiB/s (41.98–42.53 MiB/s)<br>C: 34.29 MiB (34.18–35.00 MiB)<br>S: 29.17 MiB (28.34–29.84 MiB) | 40.23 MiB/s (37.99–50.13 MiB/s)<br>C: 36.50 MiB (36.12–37.07 MiB)<br>S: 30.35 MiB (30.16–30.81 MiB) | 41.11 MiB/s (40.40–43.16 MiB/s)<br>C: 35.39 MiB (35.20–35.49 MiB)<br>S: 29.26 MiB (28.94–30.04 MiB) | 46.76 MiB/s (44.89–48.70 MiB/s)<br>C: 35.22 MiB (34.18–35.80 MiB)<br>S: 30.81 MiB (30.25–31.41 MiB) | 40.65 MiB/s (38.83–40.95 MiB/s)<br>C: 8.88 MiB (8.75–8.88 MiB)<br>S: 9.25 MiB (9.12–9.25 MiB) | 76.18 MiB/s (74.54–78.11 MiB/s)<br>C: 137.88 MiB (137.87–143.32 MiB)<br>S: 98.14 MiB (93.57–98.76 MiB) |

Concurrent rust-raknet uses the default nominal MTU of 1,400 B. quic-go uses a 1,372 B UDP packet budget with path MTU discovery disabled. C KCP uses a 1,400 B UDP MTU; its physical budget is 28 B larger, but the 64 B and 800 B messages remain unfragmented in all three.

### Delivery latency during sustained load

Concurrent clients use `--loaded-rtt`: each message is timestamped before sending, then measured when its verified echo is consumed throughout the burst. This includes client and server queueing. Each run verifies 262,144 burst echoes using the same 16-message application window, workers, CPU placement and server modes; each profile runs three times.

Latency sampling runs separately from throughput; its instrumented throughput is not mixed into the tables above. RSS includes the client's retained RTT samples, aggregation and sorting.

| Payload | Connections | Injected loss | TCP<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (`send`)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (`send` + `send-policy`)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (batch APIs + `send-policy`)<br>RTT median / 99th (range)<br>RSS: C / S | C KCP<br>RTT median / 99th (range)<br>RSS: C / S | quic-go<br>RTT median / 99th (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 connections | 0% | 50%: 0.91 ms (0.91–0.97 ms)<br>99%: 5.29 ms (4.16–5.79 ms)<br>C: 11.46 MiB (11.45–11.65 MiB)<br>S: 3.41 MiB (3.39–3.46 MiB) | 50%: 1.02 ms (0.94–1.11 ms)<br>99%: 3.00 ms (2.92–7.79 ms)<br>C: 13.38 MiB (13.34–13.43 MiB)<br>S: 5.73 MiB (5.53–5.80 MiB) | 50%: 0.47 ms (0.44–0.49 ms)<br>99%: 4.45 ms (3.81–4.86 ms)<br>C: 13.44 MiB (13.32–13.48 MiB)<br>S: 5.89 MiB (5.59–6.05 MiB) | 50%: 1.05 ms (1.00–1.08 ms)<br>99%: 3.68 ms (3.08–4.17 ms)<br>C: 13.46 MiB (13.43–13.57 MiB)<br>S: 5.82 MiB (5.69–5.88 MiB) | 50%: 0.41 ms (0.36–0.45 ms)<br>99%: 1.63 ms (1.52–2.11 ms)<br>C: 13.44 MiB (13.29–13.66 MiB)<br>S: 6.05 MiB (6.05–6.17 MiB) | 50%: 1.37 ms (1.30–1.43 ms)<br>99%: 5.92 ms (5.19–6.45 ms)<br>C: 6.21 MiB (6.09–6.38 MiB)<br>S: 2.38 MiB (2.25–2.50 MiB) | 50%: 0.34 ms (0.33–0.35 ms)<br>99%: 2.15 ms (1.86–2.98 ms)<br>C: 31.70 MiB (31.02–34.45 MiB)<br>S: 17.19 MiB (17.14–17.20 MiB) |
| 64 B | 1,024 connections | 0% | 50%: 17.00 ms (16.35–20.55 ms)<br>99%: 38.89 ms (35.81–41.34 ms)<br>C: 16.45 MiB (16.40–17.46 MiB)<br>S: 4.03 MiB (3.91–4.09 MiB) | 50%: 17.64 ms (17.44–17.82 ms)<br>99%: 54.03 ms (33.19–85.12 ms)<br>C: 36.50 MiB (36.35–36.61 MiB)<br>S: 23.05 MiB (22.97–23.55 MiB) | 50%: 5.92 ms (5.17–7.24 ms)<br>99%: 17.86 ms (16.15–19.01 ms)<br>C: 38.12 MiB (38.12–38.15 MiB)<br>S: 24.21 MiB (24.21–24.46 MiB) | 50%: 16.03 ms (15.72–16.09 ms)<br>99%: 100.53 ms (98.57–105.22 ms)<br>C: 36.65 MiB (36.41–36.68 MiB)<br>S: 24.65 MiB (24.40–24.88 MiB) | 50%: 4.77 ms (4.54–5.04 ms)<br>99%: 19.28 ms (16.26–22.82 ms)<br>C: 38.04 MiB (37.96–38.27 MiB)<br>S: 24.68 MiB (24.47–24.76 MiB) | 50%: 6.93 ms (6.85–8.06 ms)<br>99%: 108.46 ms (107.70–110.22 ms)<br>C: 10.82 MiB (10.69–10.94 MiB)<br>S: 9.25 MiB (9.25–9.25 MiB) | 50%: 9.09 ms (8.77–9.91 ms)<br>99%: 20.50 ms (15.12–24.04 ms)<br>C: 135.82 MiB (135.01–139.26 MiB)<br>S: 91.45 MiB (90.82–92.95 MiB) |
| 64 B | 64 connections | 1% | 50%: 0.66 ms (0.65–0.68 ms)<br>99%: 6.63 ms (3.90–6.67 ms)<br>C: 11.78 MiB (11.59–11.87 MiB)<br>S: 3.33 MiB (3.20–3.48 MiB) | 50%: 0.93 ms (0.93–1.06 ms)<br>99%: 5.95 ms (5.68–6.34 ms)<br>C: 13.67 MiB (13.63–13.74 MiB)<br>S: 5.96 MiB (5.74–6.07 MiB) | 50%: 0.90 ms (0.88–0.93 ms)<br>99%: 4.69 ms (4.45–4.93 ms)<br>C: 13.71 MiB (13.54–13.73 MiB)<br>S: 6.17 MiB (6.16–6.18 MiB) | 50%: 0.97 ms (0.92–1.00 ms)<br>99%: 5.37 ms (5.21–5.81 ms)<br>C: 13.61 MiB (13.57–13.67 MiB)<br>S: 6.01 MiB (5.99–6.03 MiB) | 50%: 0.96 ms (0.91–1.04 ms)<br>99%: 5.05 ms (3.96–5.24 ms)<br>C: 13.79 MiB (13.77–13.83 MiB)<br>S: 6.12 MiB (6.06–6.13 MiB) | 50%: 1.26 ms (1.20–1.38 ms)<br>99%: 7.14 ms (5.69–8.50 ms)<br>C: 6.20 MiB (6.19–6.28 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.11 ms (0.08–0.12 ms)<br>99%: 27.27 ms (27.18–27.30 ms)<br>C: 31.87 MiB (30.76–33.14 MiB)<br>S: 17.39 MiB (17.10–17.88 MiB) |
| 64 B | 1,024 connections | 1% | 50%: 16.74 ms (16.20–17.32 ms)<br>99%: 35.64 ms (33.99–40.71 ms)<br>C: 17.28 MiB (16.58–17.44 MiB)<br>S: 4.02 MiB (4.01–4.11 MiB) | 50%: 16.33 ms (16.03–17.67 ms)<br>99%: 89.30 ms (69.20–94.48 ms)<br>C: 37.62 MiB (35.47–37.67 MiB)<br>S: 24.96 MiB (24.45–25.02 MiB) | 50%: 12.30 ms (11.03–13.47 ms)<br>99%: 48.59 ms (38.75–68.93 ms)<br>C: 38.56 MiB (38.27–38.65 MiB)<br>S: 25.69 MiB (25.44–25.82 MiB) | 50%: 16.92 ms (16.90–17.35 ms)<br>99%: 58.16 ms (53.54–81.28 ms)<br>C: 37.74 MiB (36.85–38.20 MiB)<br>S: 25.46 MiB (25.38–25.70 MiB) | 50%: 12.38 ms (10.43–13.56 ms)<br>99%: 46.75 ms (38.17–52.41 ms)<br>C: 37.84 MiB (37.37–38.78 MiB)<br>S: 26.14 MiB (26.06–26.26 MiB) | 50%: 10.39 ms (7.30–15.26 ms)<br>99%: 127.16 ms (118.57–136.22 ms)<br>C: 10.69 MiB (10.69–10.82 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 50%: 8.95 ms (8.53–10.60 ms)<br>99%: 65.93 ms (48.74–71.05 ms)<br>C: 140.26 MiB (134.89–142.89 MiB)<br>S: 89.75 MiB (88.20–90.76 MiB) |
| 800 B | 64 connections | 0% | 50%: 1.16 ms (1.14–1.19 ms)<br>99%: 5.62 ms (3.61–6.01 ms)<br>C: 11.75 MiB (11.74–11.97 MiB)<br>S: 3.32 MiB (3.30–3.34 MiB) | 50%: 1.26 ms (1.17–1.39 ms)<br>99%: 8.01 ms (7.68–8.59 ms)<br>C: 14.76 MiB (14.64–14.84 MiB)<br>S: 7.20 MiB (7.12–7.63 MiB) | 50%: 1.16 ms (1.16–1.26 ms)<br>99%: 7.08 ms (5.26–7.72 ms)<br>C: 15.10 MiB (14.85–15.12 MiB)<br>S: 7.45 MiB (7.28–7.63 MiB) | 50%: 1.12 ms (1.11–1.15 ms)<br>99%: 4.91 ms (4.41–9.43 ms)<br>C: 14.67 MiB (14.52–14.77 MiB)<br>S: 7.17 MiB (7.07–7.71 MiB) | 50%: 1.23 ms (1.18–1.23 ms)<br>99%: 4.90 ms (3.50–7.48 ms)<br>C: 14.95 MiB (14.83–14.97 MiB)<br>S: 7.33 MiB (7.21–7.42 MiB) | 50%: 1.10 ms (1.06–1.35 ms)<br>99%: 23.70 ms (17.13–30.84 ms)<br>C: 6.95 MiB (6.78–6.97 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 50%: 3.09 ms (3.03–3.10 ms)<br>99%: 9.96 ms (9.08–10.89 ms)<br>C: 38.99 MiB (36.68–40.48 MiB)<br>S: 30.52 MiB (30.01–33.09 MiB) |
| 800 B | 1,024 connections | 0% | 50%: 21.41 ms (20.62–26.02 ms)<br>99%: 40.69 ms (40.26–88.40 ms)<br>C: 20.19 MiB (19.71–20.35 MiB)<br>S: 4.96 MiB (4.82–5.03 MiB) | 50%: 16.63 ms (12.34–17.05 ms)<br>99%: 119.40 ms (114.39–160.62 ms)<br>C: 50.56 MiB (50.16–50.70 MiB)<br>S: 34.45 MiB (34.35–34.50 MiB) | 50%: 13.61 ms (12.15–21.93 ms)<br>99%: 187.28 ms (42.55–206.00 ms)<br>C: 50.86 MiB (50.79–52.08 MiB)<br>S: 36.60 MiB (35.46–37.78 MiB) | 50%: 17.78 ms (16.31–18.46 ms)<br>99%: 118.04 ms (109.44–131.81 ms)<br>C: 50.73 MiB (50.50–51.07 MiB)<br>S: 35.01 MiB (34.98–36.59 MiB) | 50%: 15.19 ms (14.80–19.37 ms)<br>99%: 122.03 ms (95.69–207.62 ms)<br>C: 52.02 MiB (51.84–52.64 MiB)<br>S: 36.10 MiB (35.41–36.42 MiB) | 50%: 17.59 ms (16.12–17.80 ms)<br>99%: 189.30 ms (122.63–215.00 ms)<br>C: 22.19 MiB (22.07–22.19 MiB)<br>S: 20.13 MiB (19.50–20.75 MiB) | 50%: 69.40 ms (68.71–74.07 ms)<br>99%: 187.23 ms (126.03–233.35 ms)<br>C: 231.64 MiB (224.52–241.82 MiB)<br>S: 164.76 MiB (162.76–168.14 MiB) |
| 800 B | 64 connections | 1% | 50%: 0.69 ms (0.56–0.77 ms)<br>99%: 6.64 ms (4.28–7.94 ms)<br>C: 11.75 MiB (11.73–11.98 MiB)<br>S: 3.51 MiB (3.22–3.58 MiB) | 50%: 1.06 ms (1.04–1.15 ms)<br>99%: 5.84 ms (5.10–6.58 ms)<br>C: 14.89 MiB (14.81–15.00 MiB)<br>S: 7.45 MiB (7.28–7.46 MiB) | 50%: 1.08 ms (0.97–1.12 ms)<br>99%: 5.62 ms (3.66–5.70 ms)<br>C: 15.17 MiB (15.04–15.23 MiB)<br>S: 7.57 MiB (7.54–7.94 MiB) | 50%: 1.07 ms (1.00–1.09 ms)<br>99%: 6.14 ms (5.55–6.87 ms)<br>C: 14.91 MiB (14.86–15.12 MiB)<br>S: 7.34 MiB (7.29–7.88 MiB) | 50%: 1.11 ms (1.04–1.34 ms)<br>99%: 5.11 ms (4.99–6.43 ms)<br>C: 15.18 MiB (15.09–15.33 MiB)<br>S: 7.68 MiB (7.27–8.07 MiB) | 50%: 1.15 ms (1.11–1.21 ms)<br>99%: 16.06 ms (11.78–31.16 ms)<br>C: 6.84 MiB (6.66–6.86 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.13 ms (3.13–3.19 ms)<br>99%: 19.22 ms (18.83–20.43 ms)<br>C: 40.45 MiB (36.31–42.92 MiB)<br>S: 29.07 MiB (28.70–32.08 MiB) |
| 800 B | 1,024 connections | 1% | 50%: 20.62 ms (18.69–25.53 ms)<br>99%: 53.56 ms (50.23–148.66 ms)<br>C: 18.92 MiB (18.27–20.41 MiB)<br>S: 4.86 MiB (4.81–4.89 MiB) | 50%: 19.83 ms (11.15–21.07 ms)<br>99%: 99.99 ms (98.24–177.44 ms)<br>C: 51.28 MiB (49.68–51.31 MiB)<br>S: 36.79 MiB (35.71–37.23 MiB) | 50%: 14.21 ms (11.76–25.77 ms)<br>99%: 132.55 ms (117.70–177.69 ms)<br>C: 51.98 MiB (50.08–55.20 MiB)<br>S: 37.01 MiB (36.27–38.05 MiB) | 50%: 18.70 ms (13.54–19.96 ms)<br>99%: 113.76 ms (100.94–172.58 ms)<br>C: 50.99 MiB (49.09–51.76 MiB)<br>S: 36.54 MiB (35.43–37.35 MiB) | 50%: 19.49 ms (17.25–21.00 ms)<br>99%: 113.99 ms (84.43–120.12 ms)<br>C: 50.91 MiB (50.61–53.35 MiB)<br>S: 37.81 MiB (37.49–38.38 MiB) | 50%: 18.19 ms (17.60–21.47 ms)<br>99%: 124.72 ms (114.22–353.38 ms)<br>C: 22.32 MiB (22.07–22.32 MiB)<br>S: 20.13 MiB (19.50–20.50 MiB) | 50%: 67.66 ms (65.72–68.10 ms)<br>99%: 289.12 ms (282.72–302.63 ms)<br>C: 245.05 MiB (243.28–248.14 MiB)<br>S: 183.63 MiB (180.06–195.50 MiB) |

### Reading the results

- At 64 B and 1,024 connections without injected loss, default ordinary sends deliver **46.73 MiB/s**, and batch APIs **142.95 MiB/s**. TCP, C KCP and quic-go deliver **54.18, 44.69 and 107.37 MiB/s**, respectively.
- For one connection, 800 B messages and 5% loss, default ordinary sends deliver **140.90 MiB/s**. TCP, C KCP and quic-go deliver **1.03, 104.70 and 9.96 MiB/s**, respectively. These describe this configured reliable ordered echo workload.
- The policy is not faster in every profile. At 800 B and 1,024 connections without injected loss, ordinary sends change from **464.75 to 486.67 MiB/s**; batch APIs change from **481.91 to 423.69 MiB/s**.
- At 64 B and 1,024 connections without injected loss, enabling the policy changes ordinary loaded median RTT from **17.64 to 16.03 ms**, but 99th-percentile RTT from **54.03 to 100.53 ms**. Compare throughput, slow-message latency and memory together.

All profiles use reliable ordered traffic. They do not test mixed reliable/unreliable scheduling, abrupt bandwidth changes or congestion fairness. `send-policy` remains optional. This matrix cannot replace mixed-traffic validation and is not a controlled before/after comparison with an older revision.

UDP receive buffers can overflow even without injected loss. Namespace-local UDP error counters and qdisc statistics were recorded for each run. Short workloads, random loss and CPU scheduling produce variation; small differences are not evidence of statistical significance or a universal ranking.

Maximum namespace UDP receive-buffer drops in one throughput run: rust-raknet (`send`): 175,509 datagrams; rust-raknet (batch APIs): 407,363 datagrams; rust-raknet (`send` + `send-policy`): 287,825 datagrams; rust-raknet (batch APIs + `send-policy`): 295,212 datagrams; C KCP: 417,267 datagrams; quic-go: 7,000 datagrams.

All **623 measurements** completed with verified echoes: 168 single-connection throughput runs, 35 sparse-latency runs, 252 concurrent-throughput runs and 168 loaded-latency runs.

Default drivers, API flags and workloads are documented in [the benchmark README](examples/test_benchmark/README.md), [the C KCP adapters](examples/test_benchmark/kcp/README.md) and [the quic-go adapters](examples/test_benchmark/quic/README.md). Policy columns were built in a separate validation copy, adding calls to apply `SendOptions::default()` to each connected or accepted socket.

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
