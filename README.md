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

[Features](#features) · [Wiki](https://github.com/b23r0/rust-raknet/wiki) · [Get started](#get-started) · [Benchmarks](#benchmarks) · [Contributing](#contributing)

</div>

`rust-raknet` is a high-performance asynchronous networking library written in Rust, implementing a Reliable UDP protocol for real-time messaging.

## Features

- **Five delivery modes.** Send best-effort updates, discard stale state, deliver reliable events, or keep messages ordered on independent channels.
- **Async client and server.** Tokio-based connect, listen, accept, send and receive APIs, with IPv4 and IPv6 address encoding.
- **Blocking client and server.** The optional `blocking` feature provides synchronous connect, listen, accept, send, receive and timeout APIs backed by a shared Tokio runtime. Background workers keep ACKs and retries running between calls. [Usage](#blocking-api).
- **Message boundaries.** Receive complete application messages rather than reconstructing them from a byte stream. Large `ReliableOrdered` messages are fragmented and reassembled automatically.
- **Loss recovery.** ACK/NACK ranges, selective retries and ACK-gap fast retransmission, with RTT-based retry timers and retransmission backoff.
- **Tail-loss probing.** Optional `recovery-policy` settings can reduce waits after isolated tail loss. Probe whole datagrams, retain normal RTO fallback, or enable deadline-driven retries independently. [Configuration and trade-offs](https://github.com/b23r0/rust-raknet/wiki/Recovery-Policy-EN).
- **Owned buffer forwarding.** `Bytes` send/receive APIs share immutable payload storage with send and retry queues, reducing copies in relays.
- **Bounded queues and backpressure.** Send accounting, receive reordering and fragment assembly have limits; sustained producers wait for capacity.
- **Optional send policy.** The `send-policy` feature exposes per-connection flight limits, reliable/unreliable queue budgets and flush budgets. It is opt-in; see the measured [performance trade-offs](#performance-trade-offs).
- **Server tuning.** Configure the nominal MTU, accept backlog and UDP receive buffer. Optional Linux socket sharding, receive batching and idle maintenance let you tune busy listeners.
- **Discovery and lifecycle.** Unconnected ping/pong, customizable MOTD, peer RakNet version lookup, flush and close APIs.
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
rust-raknet = "1.4.0"
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

### Blocking API

The optional `blocking` feature provides synchronous adapters. Create one
`blocking::RaknetRuntime` and share it across clients and listeners. Network
workers keep ACKs, retries and heartbeats running between calls; no runtime is
created per connection. The default worker count is one and can be configured
with `with_worker_threads`.

```toml
rust-raknet = { version = "1.4.0", features = ["blocking"] }
```

```rust
use rust_raknet::{blocking::RaknetRuntime, Reliability};
use std::time::Duration;

let runtime = RaknetRuntime::new()?;
let socket = runtime.connect_timeout(
    &"127.0.0.1:19132".parse()?,
    Duration::from_secs(10),
)?;
socket.send(&[0xfe, 42], Reliability::ReliableOrdered)?;
let reply = socket.recv_timeout(Duration::from_secs(5))?;
socket.flush_timeout(Duration::from_secs(5))?;
socket.close()?;
```

Use `runtime.bind(...)`, configure the listener, then call `listen()` and
`accept()` or `accept_timeout()`. Owned buffers, batches, ordering channels and
optional send/recovery policies are also available through the blocking socket.

These methods block the calling thread. Do not use them in game update systems
or Tokio async contexts; async-context calls return `blocking::Error::AsyncContext`.
`send` does not wait for peer acknowledgement; `flush` does. A timed-out send may
already have queued data, and a timed-out batch may have sent a prefix. Retrying
can duplicate messages. Call `flush` and `close` explicitly when delivery matters;
dropping the final runtime owner stops background work without waiting for ACKs.
See the separate [blocking API benchmarks](examples/test_benchmark/BLOCKING.md)
for throughput, latency, memory and the cost of sharing a runtime across connections.

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
rust-raknet = { version = "1.4.0", features = ["send-policy"] }
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

Results from October 12, using the default policy preset; values below are
run medians. [The benchmark tables](#benchmarks) include ranges and memory.

| Reliable ordered workload | Without policy | Configured policy | Median change |
| --- | --- | --- | --- |
| One connection, 800 B, no injected loss, ordinary throughput | 186.77 MiB/s | 183.84 MiB/s | -1.6% |
| 1,024 connections, 800 B, no injected loss, ordinary throughput | 372.54 MiB/s | 395.28 MiB/s | +6.1% |
| 1,024 connections, 800 B, no injected loss, batch throughput | 413.36 MiB/s | 381.62 MiB/s | -7.7% |
| 1,024 connections, 64 B, no injected loss, ordinary loaded 99th-percentile RTT | 94.65 ms | 101.42 ms | +7.1% |

These three-repeat measurements show trade-offs, not statistical significance.
Policy accounting and scheduling can add work and change the timing of admitted
messages and bursts. This can affect throughput and slow-message latency even
without changing the wire protocol; the measurements do not isolate a single
cause for each difference.

Earlier mixed-traffic validation (October 3, 2026) used eight connections in separate processes,
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
`cargo run --example NAME`. Subdirectories containing a `Cargo.toml`
are standalone Cargo projects; run them with
`cargo run --manifest-path examples/PROJECT/Cargo.toml`.

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

## Recovery tuning

Enable `recovery-policy`, then configure each connected or accepted socket:

```toml
rust-raknet = { version = "1.4.0", features = ["recovery-policy"] }
```

```rust
use rust_raknet::RecoveryOptions;
use std::time::Duration;

socket.set_recovery_options(RecoveryOptions {
    tail_probe_min_delay: Some(Duration::from_millis(10)),
    ..RecoveryOptions::default()
}).await?;
```

All options are off by default. Probing implies deadline-driven maintenance and waits at least the configured minimum and 1.5 times the smoothed RTT. It preserves the original RTO deadline; 10 ms is not a delivery guarantee. Earlier recovery can reduce isolated tail-loss waits, but can reduce sustained throughput or increase congestion latency. The benchmark tables below measure **1.4.0**, without recovery-policy.

See the [recovery guide](https://github.com/b23r0/rust-raknet/wiki/Recovery-Policy-EN) for deadline-only retries, RTT-progress backoff, disabling options and measured trade-offs; the [configuration reference](https://github.com/b23r0/rust-raknet/wiki/Configuration-Reference-EN) lists all public tuning parameters.

## Benchmarks

These measurements compare this **rust-raknet implementation** with TCP, the official C KCP core and quic-go. Ordinary sends and batch APIs are measured with the default build and with an explicitly configured `send-policy`. They do not measure the RakNet protocol in general.

**Measured on October 12, 2026.** Intel Core i7-9700F (8 cores), Linux x86_64, kernel `7.0.11-76070011-generic`; Rust 1.98.1, Tokio 1.53.2 and Go 1.27.2; optimized builds. All processes ran in a task copy with private caches and a private loopback network namespace. Loopback MTU was 1,500 B, GSO/GRO was limited to one packet, netem's queue limit was 100,000 packets, and processes used `nice 10`. Host network settings were unchanged.

### Implementations and API modes

| Column | GitHub source | Tested version / configuration |
| --- | --- | --- |
| TCP | [TCP echo driver](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark), [Tokio](https://github.com/tokio-rs/tokio) | Driver 0.1.0; Tokio 1.53.2; Linux TCP stack 7.0.11-76070011-generic |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 1.4.0; source revision `1199ac7`; default build |
| rust-raknet (batch APIs) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | The same default library build; explicit batch APIs |
| rust-raknet (`send` + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | Same source; feature enabled; both endpoints call `set_send_options(SendOptions::default())` |
| rust-raknet (batch APIs + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | Same configured build; explicit batch APIs |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | Pinned commit `b1a7a21`; upstream C core unchanged |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.2 |

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
| No injected loss | 800 B / 200,000 messages | 155.70 MiB/s (153.20–158.29 MiB/s)<br>C: 3.47 MiB (3.20–3.63 MiB)<br>S: 3.40 MiB (3.32–3.42 MiB) | 186.77 MiB/s (185.18–187.22 MiB/s)<br>C: 3.70 MiB (3.62–3.88 MiB)<br>S: 3.73 MiB (3.72–4.06 MiB) | 188.15 MiB/s (183.18–192.66 MiB/s)<br>C: 3.86 MiB (3.71–3.92 MiB)<br>S: 3.93 MiB (3.82–4.03 MiB) | 183.84 MiB/s (179.94–184.51 MiB/s)<br>C: 3.85 MiB (3.82–4.00 MiB)<br>S: 3.85 MiB (3.81–3.88 MiB) | 189.79 MiB/s (188.38–189.79 MiB/s)<br>C: 3.96 MiB (3.80–4.00 MiB)<br>S: 3.82 MiB (3.77–3.84 MiB) | 151.63 MiB/s (146.52–153.23 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 115.12 MiB/s (111.58–116.03 MiB/s)<br>C: 13.97 MiB (13.93–14.16 MiB)<br>S: 13.96 MiB (13.72–14.06 MiB) |
| 1% loss | 800 B / 50,000 messages | 17.81 MiB/s (11.76–25.83 MiB/s)<br>C: 3.70 MiB (3.61–3.74 MiB)<br>S: 3.49 MiB (3.45–3.51 MiB) | 167.31 MiB/s (153.82–168.05 MiB/s)<br>C: 3.89 MiB (3.58–4.10 MiB)<br>S: 3.90 MiB (3.79–3.93 MiB) | 158.95 MiB/s (125.90–164.43 MiB/s)<br>C: 3.80 MiB (3.71–4.02 MiB)<br>S: 3.68 MiB (3.61–3.73 MiB) | 155.70 MiB/s (150.78–158.95 MiB/s)<br>C: 3.82 MiB (3.78–3.87 MiB)<br>S: 3.88 MiB (3.66–4.06 MiB) | 167.31 MiB/s (159.61–174.19 MiB/s)<br>C: 3.70 MiB (3.69–4.09 MiB)<br>S: 3.96 MiB (3.90–4.00 MiB) | 131.80 MiB/s (126.00–142.75 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 44.67 MiB/s (39.12–75.04 MiB/s)<br>C: 13.70 MiB (13.20–13.95 MiB)<br>S: 13.51 MiB (13.38–13.58 MiB) |
| 5% loss | 800 B / 50,000 messages | 1.03 MiB/s (1.01–1.20 MiB/s)<br>C: 3.57 MiB (3.54–3.63 MiB)<br>S: 3.48 MiB (3.36–3.50 MiB) | 146.72 MiB/s (146.16–156.98 MiB/s)<br>C: 3.68 MiB (3.59–3.79 MiB)<br>S: 4.01 MiB (3.73–4.03 MiB) | 106.85 MiB/s (49.10–141.29 MiB/s)<br>C: 3.92 MiB (3.66–3.92 MiB)<br>S: 3.76 MiB (3.73–3.86 MiB) | 155.70 MiB/s (110.25–160.28 MiB/s)<br>C: 3.98 MiB (3.92–3.98 MiB)<br>S: 3.78 MiB (3.64–3.96 MiB) | 145.05 MiB/s (143.41–151.38 MiB/s)<br>C: 3.76 MiB (3.71–3.96 MiB)<br>S: 3.88 MiB (3.76–4.08 MiB) | 102.06 MiB/s (95.89–107.54 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 8.07 MiB/s (8.00–8.96 MiB/s)<br>C: 13.92 MiB (13.90–14.17 MiB)<br>S: 13.99 MiB (13.45–14.01 MiB) |
| No injected loss | 64 B / 300,000 messages | 14.36 MiB/s (14.25–14.54 MiB/s)<br>C: 3.63 MiB (3.52–3.66 MiB)<br>S: 3.40 MiB (3.29–3.48 MiB) | 15.94 MiB/s (15.76–15.95 MiB/s)<br>C: 3.68 MiB (3.63–3.74 MiB)<br>S: 3.73 MiB (3.61–3.74 MiB) | 47.07 MiB/s (20.95–50.03 MiB/s)<br>C: 3.66 MiB (3.66–3.79 MiB)<br>S: 3.79 MiB (3.55–3.85 MiB) | 15.10 MiB/s (15.01–15.73 MiB/s)<br>C: 3.73 MiB (3.71–3.75 MiB)<br>S: 3.75 MiB (3.69–3.93 MiB) | 27.96 MiB/s (17.73–49.49 MiB/s)<br>C: 3.82 MiB (3.71–3.89 MiB)<br>S: 3.79 MiB (3.60–3.80 MiB) | 12.13 MiB/s (11.66–12.28 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 29.02 MiB/s (18.59–30.44 MiB/s)<br>C: 13.76 MiB (13.70–13.77 MiB)<br>S: 13.33 MiB (13.13–13.38 MiB) |
| 1% loss | 64 B / 100,000 messages | 2.62 MiB/s (2.26–2.62 MiB/s)<br>C: 3.50 MiB (3.42–3.52 MiB)<br>S: 3.28 MiB (3.16–3.41 MiB) | 14.60 MiB/s (14.06–15.65 MiB/s)<br>C: 3.80 MiB (3.68–3.83 MiB)<br>S: 3.64 MiB (3.62–3.71 MiB) | 15.30 MiB/s (14.40–15.85 MiB/s)<br>C: 3.67 MiB (3.67–3.67 MiB)<br>S: 3.76 MiB (3.69–3.78 MiB) | 14.85 MiB/s (14.64–15.11 MiB/s)<br>C: 3.71 MiB (3.70–3.79 MiB)<br>S: 3.82 MiB (3.62–3.91 MiB) | 14.96 MiB/s (10.63–15.03 MiB/s)<br>C: 3.82 MiB (3.71–3.90 MiB)<br>S: 3.78 MiB (3.73–3.82 MiB) | 11.39 MiB/s (11.16–11.55 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 24.55 MiB/s (10.66–24.69 MiB/s)<br>C: 13.07 MiB (12.07–13.07 MiB)<br>S: 11.82 MiB (11.63–11.88 MiB) |
| 5% loss | 64 B / 100,000 messages | 0.13 MiB/s (0.12–0.14 MiB/s)<br>C: 3.48 MiB (3.46–3.78 MiB)<br>S: 3.48 MiB (3.32–3.48 MiB) | 13.56 MiB/s (12.88–14.00 MiB/s)<br>C: 3.66 MiB (3.60–3.71 MiB)<br>S: 3.74 MiB (3.59–3.77 MiB) | 13.38 MiB/s (13.13–14.00 MiB/s)<br>C: 3.66 MiB (3.48–3.69 MiB)<br>S: 3.78 MiB (3.73–3.91 MiB) | 13.62 MiB/s (10.78–13.72 MiB/s)<br>C: 3.73 MiB (3.54–3.87 MiB)<br>S: 3.68 MiB (3.66–3.70 MiB) | 13.53 MiB/s (12.01–13.56 MiB/s)<br>C: 3.65 MiB (3.62–3.72 MiB)<br>S: 3.80 MiB (3.65–3.85 MiB) | 9.85 MiB/s (9.84–10.02 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 4.91 MiB/s (4.26–5.26 MiB/s)<br>C: 12.63 MiB (12.45–12.70 MiB)<br>S: 12.13 MiB (12.07–12.25 MiB) |
| No injected loss | 4,096 B / 50,000 messages | 350.65 MiB/s (350.02–352.55 MiB/s)<br>C: 3.60 MiB (3.56–3.67 MiB)<br>S: 3.50 MiB (3.48–3.53 MiB) | 282.24 MiB/s (279.02–285.54 MiB/s)<br>C: 4.81 MiB (4.37–4.89 MiB)<br>S: 4.94 MiB (4.93–5.02 MiB) | 283.88 MiB/s (105.98–286.38 MiB/s)<br>C: 4.66 MiB (4.66–4.95 MiB)<br>S: 5.03 MiB (4.81–5.21 MiB) | 282.24 MiB/s (281.03–287.22 MiB/s)<br>C: 4.90 MiB (4.84–4.94 MiB)<br>S: 5.07 MiB (4.92–5.14 MiB) | 286.38 MiB/s (164.40–290.21 MiB/s)<br>C: 5.03 MiB (4.88–5.04 MiB)<br>S: 5.08 MiB (4.87–5.12 MiB) | 239.75 MiB/s (223.99–249.66 MiB/s)<br>C: 1.86 MiB (1.86–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 212.51 MiB/s (210.72–215.61 MiB/s)<br>C: 14.21 MiB (13.78–14.42 MiB)<br>S: 13.75 MiB (13.67–13.98 MiB) |
| 10 ms RTT + 1% loss | 800 B / 3,000 messages | 1.27 MiB/s (1.06–1.35 MiB/s)<br>C: 3.69 MiB (3.66–3.77 MiB)<br>S: 3.52 MiB (3.38–3.55 MiB) | 3.21 MiB/s (3.15–3.28 MiB/s)<br>C: 3.78 MiB (3.61–3.81 MiB)<br>S: 3.77 MiB (3.71–3.91 MiB) | 2.89 MiB/s (2.78–2.90 MiB/s)<br>C: 3.78 MiB (3.75–3.94 MiB)<br>S: 3.66 MiB (3.58–3.70 MiB) | 3.04 MiB/s (2.76–3.25 MiB/s)<br>C: 3.82 MiB (3.76–3.87 MiB)<br>S: 3.81 MiB (3.81–3.93 MiB) | 2.95 MiB/s (2.90–3.18 MiB/s)<br>C: 3.82 MiB (3.69–3.82 MiB)<br>S: 3.67 MiB (3.49–3.69 MiB) | 2.96 MiB/s (2.89–3.15 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 1.78 MiB/s (1.66–1.97 MiB/s)<br>C: 11.70 MiB (11.32–11.70 MiB)<br>S: 11.38 MiB (11.13–11.88 MiB) |

Single-connection rust-raknet uses a 1,428 B nominal MTU, including 28 B of IPv4/UDP overhead; C KCP and quic-go use a 1,400 B UDP packet budget. The 4,096 B profile triggers fragmentation in rust-raknet and C KCP.

### Sparse request/echo latency

800 B messages, no injected loss; five runs, each with 1,000 warmups and 10,000 sequential samples. Client and server are pinned to different CPUs; each Rust/Go process has four workers, while C KCP uses one event loop. Each request waits for its echo. The batch API receives only one message per call; nothing is coalesced here.

| Implementation / API | Median RTT (range) | 95th-percentile RTT (range) | 99th-percentile RTT (range) | Peak RSS: C / S (range) |
| --- | --- | --- | --- | --- |
| TCP | 13.00 µs (12.60–13.30 µs) | 26.80 µs (18.40–29.70 µs) | 66.10 µs (33.20–150.10 µs) | C: 3.91 MiB (3.87–3.93 MiB)<br>S: 3.56 MiB (3.49–3.67 MiB) |
| rust-raknet (`send`) | 17.90 µs (17.30–18.10 µs) | 33.70 µs (30.20–35.70 µs) | 73.10 µs (46.30–264.40 µs) | C: 4.24 MiB (4.12–4.31 MiB)<br>S: 4.02 MiB (3.98–4.29 MiB) |
| rust-raknet (batch APIs) | 18.60 µs (17.50–18.80 µs) | 33.10 µs (31.40–36.70 µs) | 75.50 µs (47.30–111.20 µs) | C: 4.31 MiB (4.21–4.34 MiB)<br>S: 4.10 MiB (3.98–4.21 MiB) |
| rust-raknet (`send` + `send-policy`) | 17.40 µs (17.10–17.60 µs) | 31.40 µs (30.10–34.70 µs) | 48.60 µs (43.60–110.00 µs) | C: 4.32 MiB (4.09–4.39 MiB)<br>S: 4.03 MiB (3.96–4.05 MiB) |
| rust-raknet (batch APIs + `send-policy`) | 17.50 µs (17.20–18.30 µs) | 32.40 µs (21.80–35.50 µs) | 50.10 µs (46.30–205.50 µs) | C: 4.27 MiB (4.24–4.33 MiB)<br>S: 4.03 MiB (3.93–4.14 MiB) |
| C KCP | 10.20 µs (10.10–10.30 µs) | 24.00 µs (18.10–24.40 µs) | 36.20 µs (33.40–66.60 µs) | C: 1.87 MiB (1.87–1.88 MiB)<br>S: 1.87 MiB (1.75–1.99 MiB) |
| quic-go | 62.50 µs (62.30–63.90 µs) | 108.30 µs (97.30–141.70 µs) | 211.40 µs (126.90–449.30 µs) | C: 13.45 MiB (13.20–13.45 MiB)<br>S: 10.94 MiB (10.88–11.13 MiB) |

These measure the sparse API path, not burst delivery latency. Concurrent throughput's pre-burst RTT samples are also not used as loaded latency below.

### Concurrent throughput

Each run verifies 1,048,576 ordered burst echoes with a 16-message application window per connection. Connection IDs, message IDs, complete payloads and ordering are checked. Twenty sequential samples per connection precede a common start barrier; setup and warmup are excluded from the throughput timer. Connections stay open until every burst finishes.

All seven columns use four workers and four server listeners/receive sockets with `SO_REUSEPORT`. Servers are pinned to CPUs 0–3 and clients to CPUs 4–7. Affinity does not reserve CPUs exclusively.

| Payload | Connections | Injected loss | TCP<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>Throughput (range)<br>RSS: C / S | rust-raknet (`send` + `send-policy`)<br>Throughput (range)<br>RSS: C / S | rust-raknet (batch APIs + `send-policy`)<br>Throughput (range)<br>RSS: C / S | C KCP<br>Throughput (range)<br>RSS: C / S | quic-go<br>Throughput (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 connections | 0% | 556.05 MiB/s (410.64–642.81 MiB/s)<br>C: 4.07 MiB (4.06–4.29 MiB)<br>S: 3.42 MiB (3.23–3.54 MiB) | 432.99 MiB/s (385.15–515.15 MiB/s)<br>C: 7.41 MiB (7.39–7.44 MiB)<br>S: 7.78 MiB (7.75–8.02 MiB) | 533.52 MiB/s (405.64–566.27 MiB/s)<br>C: 7.41 MiB (7.34–7.91 MiB)<br>S: 7.91 MiB (7.72–8.25 MiB) | 548.45 MiB/s (464.52–549.28 MiB/s)<br>C: 7.06 MiB (6.88–7.13 MiB)<br>S: 7.81 MiB (7.43–7.86 MiB) | 511.69 MiB/s (347.19–540.66 MiB/s)<br>C: 7.56 MiB (7.39–7.69 MiB)<br>S: 8.07 MiB (7.77–8.41 MiB) | 369.80 MiB/s (363.04–387.54 MiB/s)<br>C: 3.00 MiB (2.88–3.25 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 213.74 MiB/s (180.33–215.28 MiB/s)<br>C: 29.08 MiB (28.82–29.53 MiB)<br>S: 39.49 MiB (34.58–42.39 MiB) |
| 800 B | 256 connections | 0% | 538.72 MiB/s (537.62–568.30 MiB/s)<br>C: 5.82 MiB (4.90–5.95 MiB)<br>S: 3.70 MiB (3.43–3.80 MiB) | 437.67 MiB/s (357.31–440.04 MiB/s)<br>C: 15.65 MiB (15.27–15.83 MiB)<br>S: 14.26 MiB (14.16–14.64 MiB) | 445.78 MiB/s (436.02–468.97 MiB/s)<br>C: 16.65 MiB (16.59–16.74 MiB)<br>S: 14.96 MiB (14.85–15.03 MiB) | 441.06 MiB/s (439.33–519.85 MiB/s)<br>C: 15.70 MiB (15.68–15.97 MiB)<br>S: 14.41 MiB (13.92–14.97 MiB) | 455.89 MiB/s (415.13–469.25 MiB/s)<br>C: 16.51 MiB (16.14–17.27 MiB)<br>S: 15.01 MiB (14.52–15.35 MiB) | 345.96 MiB/s (330.21–358.24 MiB/s)<br>C: 6.38 MiB (6.38–6.50 MiB)<br>S: 6.88 MiB (6.75–7.00 MiB) | 148.73 MiB/s (147.87–170.36 MiB/s)<br>C: 77.93 MiB (75.91–82.06 MiB)<br>S: 109.27 MiB (100.62–116.70 MiB) |
| 800 B | 1,024 connections | 0% | 467.22 MiB/s (455.60–525.70 MiB/s)<br>C: 11.78 MiB (9.52–12.20 MiB)<br>S: 4.89 MiB (4.58–4.89 MiB) | 372.54 MiB/s (347.08–487.01 MiB/s)<br>C: 46.31 MiB (45.80–46.84 MiB)<br>S: 41.92 MiB (40.86–41.95 MiB) | 413.36 MiB/s (394.91–441.03 MiB/s)<br>C: 48.41 MiB (47.44–48.72 MiB)<br>S: 42.28 MiB (41.79–42.38 MiB) | 395.28 MiB/s (333.46–423.21 MiB/s)<br>C: 46.93 MiB (46.00–46.96 MiB)<br>S: 40.87 MiB (39.96–42.48 MiB) | 381.62 MiB/s (362.67–429.23 MiB/s)<br>C: 48.21 MiB (48.11–48.66 MiB)<br>S: 42.17 MiB (41.21–44.30 MiB) | 286.75 MiB/s (268.24–327.30 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 21.00 MiB (20.88–21.00 MiB) | 141.31 MiB/s (136.13–152.38 MiB/s)<br>C: 244.89 MiB (243.89–249.58 MiB)<br>S: 254.45 MiB (250.95–266.39 MiB) |
| 800 B | 2,048 connections | 0% | 475.75 MiB/s (450.53–536.35 MiB/s)<br>C: 21.03 MiB (15.96–21.16 MiB)<br>S: 6.52 MiB (6.32–6.59 MiB) | 322.52 MiB/s (260.15–386.99 MiB/s)<br>C: 85.75 MiB (85.56–86.81 MiB)<br>S: 71.90 MiB (69.68–73.27 MiB) | 298.04 MiB/s (278.00–312.94 MiB/s)<br>C: 89.44 MiB (87.82–90.04 MiB)<br>S: 73.47 MiB (71.82–75.50 MiB) | 309.85 MiB/s (277.92–354.99 MiB/s)<br>C: 87.36 MiB (86.17–89.85 MiB)<br>S: 70.89 MiB (68.00–73.43 MiB) | 329.25 MiB/s (248.12–373.05 MiB/s)<br>C: 89.25 MiB (88.55–89.43 MiB)<br>S: 71.56 MiB (71.06–76.66 MiB) | 249.87 MiB/s (198.62–290.60 MiB/s)<br>C: 38.88 MiB (38.75–38.88 MiB)<br>S: 38.88 MiB (38.88–39.13 MiB) | 103.79 MiB/s (81.86–112.80 MiB/s)<br>C: 473.59 MiB (472.48–589.80 MiB)<br>S: 497.85 MiB (443.79–550.16 MiB) |
| 800 B | 64 connections | 1% | 413.68 MiB/s (352.94–457.15 MiB/s)<br>C: 4.04 MiB (3.96–4.18 MiB)<br>S: 3.27 MiB (3.22–3.46 MiB) | 457.08 MiB/s (388.06–521.29 MiB/s)<br>C: 7.55 MiB (7.32–7.55 MiB)<br>S: 8.13 MiB (7.62–8.40 MiB) | 506.71 MiB/s (433.95–523.45 MiB/s)<br>C: 7.79 MiB (7.62–7.88 MiB)<br>S: 8.28 MiB (7.96–8.47 MiB) | 500.69 MiB/s (385.72–525.45 MiB/s)<br>C: 7.26 MiB (7.12–7.65 MiB)<br>S: 7.98 MiB (7.58–9.02 MiB) | 522.91 MiB/s (503.63–558.91 MiB/s)<br>C: 7.60 MiB (7.32–7.78 MiB)<br>S: 7.93 MiB (7.64–8.17 MiB) | 329.76 MiB/s (291.94–376.96 MiB/s)<br>C: 3.00 MiB (2.88–3.01 MiB)<br>S: 3.25 MiB (3.12–3.25 MiB) | 176.69 MiB/s (147.34–208.96 MiB/s)<br>C: 29.22 MiB (28.43–29.27 MiB)<br>S: 34.60 MiB (31.64–43.68 MiB) |
| 800 B | 256 connections | 1% | 483.00 MiB/s (436.33–505.26 MiB/s)<br>C: 5.80 MiB (5.31–5.81 MiB)<br>S: 3.59 MiB (3.50–3.82 MiB) | 506.58 MiB/s (493.75–511.85 MiB/s)<br>C: 15.97 MiB (15.94–16.43 MiB)<br>S: 15.40 MiB (15.38–15.59 MiB) | 483.16 MiB/s (479.45–497.38 MiB/s)<br>C: 17.00 MiB (16.40–17.00 MiB)<br>S: 15.44 MiB (15.05–15.67 MiB) | 499.84 MiB/s (480.30–521.71 MiB/s)<br>C: 16.50 MiB (15.82–16.70 MiB)<br>S: 15.19 MiB (14.90–15.30 MiB) | 483.76 MiB/s (475.20–499.84 MiB/s)<br>C: 16.84 MiB (16.39–17.03 MiB)<br>S: 15.81 MiB (15.68–15.90 MiB) | 410.95 MiB/s (409.96–411.24 MiB/s)<br>C: 6.38 MiB (6.38–6.38 MiB)<br>S: 6.88 MiB (6.75–6.88 MiB) | 179.86 MiB/s (161.81–182.35 MiB/s)<br>C: 76.25 MiB (74.11–77.55 MiB)<br>S: 69.39 MiB (68.77–89.64 MiB) |
| 800 B | 1,024 connections | 1% | 497.67 MiB/s (463.11–503.74 MiB/s)<br>C: 12.43 MiB (10.54–12.52 MiB)<br>S: 4.92 MiB (4.81–4.95 MiB) | 454.63 MiB/s (425.51–459.25 MiB/s)<br>C: 49.32 MiB (49.00–50.56 MiB)<br>S: 42.74 MiB (41.58–43.43 MiB) | 453.92 MiB/s (445.74–456.33 MiB/s)<br>C: 50.82 MiB (50.17–51.26 MiB)<br>S: 43.96 MiB (42.48–43.99 MiB) | 424.62 MiB/s (420.96–460.39 MiB/s)<br>C: 49.23 MiB (49.00–49.65 MiB)<br>S: 42.14 MiB (41.87–44.68 MiB) | 402.12 MiB/s (384.49–461.58 MiB/s)<br>C: 48.52 MiB (48.13–52.28 MiB)<br>S: 43.18 MiB (42.58–43.42 MiB) | 346.53 MiB/s (316.37–367.14 MiB/s)<br>C: 20.38 MiB (20.38–20.38 MiB)<br>S: 19.63 MiB (19.38–20.13 MiB) | 77.76 MiB/s (76.97–83.36 MiB/s)<br>C: 315.55 MiB (314.25–348.80 MiB)<br>S: 235.21 MiB (208.95–239.70 MiB) |
| 800 B | 2,048 connections | 1% | 469.70 MiB/s (459.18–493.77 MiB/s)<br>C: 19.86 MiB (17.70–21.01 MiB)<br>S: 6.59 MiB (6.56–6.64 MiB) | 341.97 MiB/s (296.70–359.13 MiB/s)<br>C: 88.65 MiB (88.50–90.46 MiB)<br>S: 74.66 MiB (72.72–76.88 MiB) | 369.99 MiB/s (321.91–375.11 MiB/s)<br>C: 92.95 MiB (92.93–92.96 MiB)<br>S: 77.58 MiB (75.41–78.39 MiB) | 355.38 MiB/s (339.30–367.94 MiB/s)<br>C: 89.71 MiB (89.45–89.98 MiB)<br>S: 75.42 MiB (72.52–76.36 MiB) | 324.93 MiB/s (294.61–367.55 MiB/s)<br>C: 93.81 MiB (93.31–94.14 MiB)<br>S: 78.66 MiB (77.67–80.61 MiB) | 325.96 MiB/s (313.13–328.15 MiB/s)<br>C: 38.75 MiB (38.75–38.88 MiB)<br>S: 37.63 MiB (37.50–37.63 MiB) | 34.58 MiB/s (32.33–40.45 MiB/s)<br>C: 748.54 MiB (737.20–782.60 MiB)<br>S: 414.66 MiB (405.23–427.52 MiB) |
| 64 B | 64 connections | 0% | 72.39 MiB/s (67.29–73.56 MiB/s)<br>C: 3.91 MiB (3.61–3.99 MiB)<br>S: 3.27 MiB (3.20–3.35 MiB) | 51.88 MiB/s (50.32–51.93 MiB/s)<br>C: 5.88 MiB (5.63–5.89 MiB)<br>S: 6.00 MiB (5.84–6.02 MiB) | 117.59 MiB/s (104.72–118.28 MiB/s)<br>C: 5.73 MiB (5.72–5.75 MiB)<br>S: 6.25 MiB (6.04–6.44 MiB) | 47.80 MiB/s (41.30–48.46 MiB/s)<br>C: 5.73 MiB (5.73–5.83 MiB)<br>S: 5.97 MiB (5.85–6.14 MiB) | 115.16 MiB/s (110.04–118.86 MiB/s)<br>C: 5.81 MiB (5.81–5.88 MiB)<br>S: 6.05 MiB (6.05–6.20 MiB) | 32.89 MiB/s (30.82–34.91 MiB/s)<br>C: 2.38 MiB (2.26–2.38 MiB)<br>S: 2.50 MiB (2.25–2.50 MiB) | 138.29 MiB/s (130.37–139.94 MiB/s)<br>C: 21.77 MiB (21.42–22.12 MiB)<br>S: 19.87 MiB (19.07–20.01 MiB) |
| 64 B | 1,024 connections | 0% | 53.63 MiB/s (49.30–54.59 MiB/s)<br>C: 9.65 MiB (9.07–9.67 MiB)<br>S: 4.11 MiB (3.95–4.16 MiB) | 47.00 MiB/s (44.10–47.15 MiB/s)<br>C: 32.38 MiB (32.21–32.65 MiB)<br>S: 27.48 MiB (26.37–27.51 MiB) | 142.00 MiB/s (138.96–144.54 MiB/s)<br>C: 31.88 MiB (31.82–32.49 MiB)<br>S: 25.93 MiB (25.65–25.95 MiB) | 49.86 MiB/s (46.67–50.46 MiB/s)<br>C: 32.77 MiB (32.59–32.99 MiB)<br>S: 27.43 MiB (27.02–28.04 MiB) | 144.83 MiB/s (144.53–147.80 MiB/s)<br>C: 32.33 MiB (32.11–32.51 MiB)<br>S: 26.24 MiB (26.11–26.29 MiB) | 42.64 MiB/s (41.42–43.80 MiB/s)<br>C: 8.88 MiB (8.75–8.88 MiB)<br>S: 9.38 MiB (9.38–9.50 MiB) | 107.73 MiB/s (106.19–109.10 MiB/s)<br>C: 145.95 MiB (139.64–148.52 MiB)<br>S: 95.89 MiB (94.64–96.89 MiB) |
| 64 B | 64 connections | 1% | 41.23 MiB/s (40.04–49.52 MiB/s)<br>C: 3.72 MiB (3.70–3.93 MiB)<br>S: 3.30 MiB (3.27–3.32 MiB) | 46.41 MiB/s (43.05–47.78 MiB/s)<br>C: 6.01 MiB (5.88–6.09 MiB)<br>S: 6.12 MiB (6.00–6.23 MiB) | 48.29 MiB/s (42.92–48.35 MiB/s)<br>C: 5.95 MiB (5.87–6.00 MiB)<br>S: 6.45 MiB (6.21–6.51 MiB) | 45.16 MiB/s (44.55–47.56 MiB/s)<br>C: 5.84 MiB (5.80–5.98 MiB)<br>S: 6.12 MiB (6.00–6.14 MiB) | 48.95 MiB/s (45.18–49.53 MiB/s)<br>C: 6.02 MiB (6.01–6.06 MiB)<br>S: 6.36 MiB (6.24–6.38 MiB) | 33.13 MiB/s (31.62–34.48 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.25–2.50 MiB) | 64.24 MiB/s (48.30–66.69 MiB/s)<br>C: 21.68 MiB (21.57–21.98 MiB)<br>S: 19.05 MiB (18.89–19.82 MiB) |
| 64 B | 1,024 connections | 1% | 45.25 MiB/s (45.23–49.36 MiB/s)<br>C: 9.55 MiB (9.08–9.56 MiB)<br>S: 4.23 MiB (4.04–4.36 MiB) | 42.53 MiB/s (42.16–44.21 MiB/s)<br>C: 34.18 MiB (33.88–35.03 MiB)<br>S: 28.25 MiB (27.84–28.43 MiB) | 47.90 MiB/s (45.21–50.35 MiB/s)<br>C: 35.03 MiB (34.45–36.13 MiB)<br>S: 30.71 MiB (29.51–31.03 MiB) | 42.91 MiB/s (41.62–43.62 MiB/s)<br>C: 34.96 MiB (33.21–35.27 MiB)<br>S: 29.32 MiB (29.29–29.82 MiB) | 49.38 MiB/s (42.25–52.09 MiB/s)<br>C: 36.15 MiB (35.83–36.56 MiB)<br>S: 30.37 MiB (30.26–30.45 MiB) | 39.81 MiB/s (39.51–42.11 MiB/s)<br>C: 8.75 MiB (8.75–8.88 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 79.28 MiB/s (74.21–85.47 MiB/s)<br>C: 143.39 MiB (135.39–154.89 MiB)<br>S: 97.64 MiB (97.27–97.77 MiB) |

Concurrent rust-raknet uses the default nominal MTU of 1,400 B. quic-go uses a 1,372 B UDP packet budget with path MTU discovery disabled. C KCP uses a 1,400 B UDP MTU; its physical budget is 28 B larger, but the 64 B and 800 B messages remain unfragmented in all three.

### Delivery latency during sustained load

Concurrent clients use `--loaded-rtt`: each message is timestamped before sending, then measured when its verified echo is consumed throughout the burst. This includes client and server queueing. Each run verifies 262,144 burst echoes using the same 16-message application window, workers, CPU placement and server modes; each profile runs three times.

Latency sampling runs separately from throughput; its instrumented throughput is not mixed into the tables above. RSS includes the client's retained RTT samples, aggregation and sorting.

| Payload | Connections | Injected loss | TCP<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (`send`)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (batch APIs)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (`send` + `send-policy`)<br>RTT median / 99th (range)<br>RSS: C / S | rust-raknet (batch APIs + `send-policy`)<br>RTT median / 99th (range)<br>RSS: C / S | C KCP<br>RTT median / 99th (range)<br>RSS: C / S | quic-go<br>RTT median / 99th (range)<br>RSS: C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 connections | 0% | 50%: 0.96 ms (0.92–0.97 ms)<br>99%: 4.59 ms (3.79–8.62 ms)<br>C: 11.70 MiB (11.48–11.79 MiB)<br>S: 3.16 MiB (3.10–3.18 MiB) | 50%: 1.11 ms (0.96–1.13 ms)<br>99%: 4.13 ms (3.35–7.12 ms)<br>C: 13.30 MiB (13.23–13.52 MiB)<br>S: 5.79 MiB (5.59–5.94 MiB) | 50%: 0.46 ms (0.41–0.49 ms)<br>99%: 4.29 ms (3.63–5.45 ms)<br>C: 13.21 MiB (13.07–13.47 MiB)<br>S: 5.93 MiB (5.88–6.04 MiB) | 50%: 1.08 ms (1.04–1.28 ms)<br>99%: 4.23 ms (3.86–8.13 ms)<br>C: 13.50 MiB (13.45–13.59 MiB)<br>S: 5.82 MiB (5.66–5.84 MiB) | 50%: 0.46 ms (0.45–0.48 ms)<br>99%: 4.18 ms (1.05–4.20 ms)<br>C: 13.31 MiB (13.29–13.61 MiB)<br>S: 5.85 MiB (5.80–6.04 MiB) | 50%: 1.31 ms (1.30–1.32 ms)<br>99%: 10.16 ms (6.87–11.64 ms)<br>C: 6.26 MiB (6.12–6.36 MiB)<br>S: 2.50 MiB (2.25–2.50 MiB) | 50%: 0.37 ms (0.34–0.41 ms)<br>99%: 1.89 ms (1.84–3.18 ms)<br>C: 32.32 MiB (31.20–33.02 MiB)<br>S: 17.45 MiB (17.25–17.52 MiB) |
| 64 B | 1,024 connections | 0% | 50%: 18.99 ms (17.77–19.36 ms)<br>99%: 35.03 ms (32.64–35.43 ms)<br>C: 16.77 MiB (16.26–16.99 MiB)<br>S: 4.04 MiB (3.90–4.18 MiB) | 50%: 16.90 ms (15.81–18.16 ms)<br>99%: 94.65 ms (40.14–95.26 ms)<br>C: 36.48 MiB (36.36–36.76 MiB)<br>S: 23.37 MiB (22.90–24.15 MiB) | 50%: 4.70 ms (4.46–4.85 ms)<br>99%: 19.61 ms (19.57–20.56 ms)<br>C: 37.57 MiB (37.43–38.11 MiB)<br>S: 24.38 MiB (24.24–24.44 MiB) | 50%: 15.81 ms (15.25–17.53 ms)<br>99%: 101.42 ms (35.93–104.92 ms)<br>C: 36.51 MiB (36.48–36.63 MiB)<br>S: 24.37 MiB (23.33–24.41 MiB) | 50%: 4.86 ms (4.83–5.30 ms)<br>99%: 19.19 ms (18.10–21.89 ms)<br>C: 38.03 MiB (37.87–38.07 MiB)<br>S: 24.65 MiB (24.57–24.85 MiB) | 50%: 7.37 ms (6.73–8.61 ms)<br>99%: 120.25 ms (112.92–160.63 ms)<br>C: 10.69 MiB (10.69–10.94 MiB)<br>S: 9.38 MiB (9.38–9.38 MiB) | 50%: 9.01 ms (8.97–9.71 ms)<br>99%: 13.32 ms (12.90–16.47 ms)<br>C: 140.02 MiB (137.39–140.58 MiB)<br>S: 88.89 MiB (87.69–90.89 MiB) |
| 64 B | 64 connections | 1% | 50%: 0.62 ms (0.61–0.72 ms)<br>99%: 5.88 ms (5.06–6.54 ms)<br>C: 11.54 MiB (11.50–11.73 MiB)<br>S: 3.28 MiB (3.22–3.29 MiB) | 50%: 0.92 ms (0.88–1.02 ms)<br>99%: 4.97 ms (4.60–6.16 ms)<br>C: 13.48 MiB (13.46–13.79 MiB)<br>S: 5.91 MiB (5.89–6.00 MiB) | 50%: 0.92 ms (0.87–0.97 ms)<br>99%: 4.54 ms (3.68–4.82 ms)<br>C: 13.62 MiB (13.59–13.90 MiB)<br>S: 6.17 MiB (6.09–6.35 MiB) | 50%: 0.95 ms (0.91–0.96 ms)<br>99%: 6.20 ms (3.96–6.51 ms)<br>C: 13.74 MiB (13.62–13.77 MiB)<br>S: 5.92 MiB (5.86–6.22 MiB) | 50%: 0.96 ms (0.91–1.00 ms)<br>99%: 4.82 ms (4.29–4.84 ms)<br>C: 13.81 MiB (13.71–14.07 MiB)<br>S: 6.24 MiB (6.21–6.24 MiB) | 50%: 1.25 ms (1.21–1.44 ms)<br>99%: 7.37 ms (7.33–8.08 ms)<br>C: 6.34 MiB (6.15–6.36 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.10 ms (0.10–0.12 ms)<br>99%: 27.24 ms (27.23–27.36 ms)<br>C: 32.52 MiB (31.79–34.39 MiB)<br>S: 17.32 MiB (16.83–17.37 MiB) |
| 64 B | 1,024 connections | 1% | 50%: 16.30 ms (16.20–17.18 ms)<br>99%: 40.97 ms (38.60–43.32 ms)<br>C: 16.58 MiB (15.57–17.49 MiB)<br>S: 3.91 MiB (3.89–3.95 MiB) | 50%: 18.21 ms (17.97–18.25 ms)<br>99%: 61.24 ms (57.43–66.63 ms)<br>C: 37.43 MiB (37.01–37.52 MiB)<br>S: 24.75 MiB (24.47–24.75 MiB) | 50%: 12.36 ms (12.18–15.99 ms)<br>99%: 39.28 ms (36.48–96.98 ms)<br>C: 38.88 MiB (37.86–39.56 MiB)<br>S: 25.61 MiB (25.61–26.52 MiB) | 50%: 16.61 ms (16.42–18.79 ms)<br>99%: 109.33 ms (65.97–109.70 ms)<br>C: 36.77 MiB (35.29–39.10 MiB)<br>S: 26.01 MiB (25.34–26.05 MiB) | 50%: 12.45 ms (11.62–15.14 ms)<br>99%: 43.40 ms (35.63–84.78 ms)<br>C: 37.48 MiB (36.12–38.46 MiB)<br>S: 26.17 MiB (25.97–26.54 MiB) | 50%: 5.90 ms (5.03–8.88 ms)<br>99%: 161.43 ms (111.57–165.05 ms)<br>C: 10.94 MiB (10.82–10.94 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 50%: 8.92 ms (8.38–10.94 ms)<br>99%: 59.60 ms (47.84–73.50 ms)<br>C: 134.08 MiB (131.52–138.64 MiB)<br>S: 89.07 MiB (86.48–90.95 MiB) |
| 800 B | 64 connections | 0% | 50%: 1.18 ms (1.16–1.22 ms)<br>99%: 8.77 ms (5.87–9.36 ms)<br>C: 11.88 MiB (11.76–11.89 MiB)<br>S: 3.37 MiB (3.14–3.42 MiB) | 50%: 1.15 ms (1.15–1.20 ms)<br>99%: 8.05 ms (3.26–8.41 ms)<br>C: 14.66 MiB (14.49–14.85 MiB)<br>S: 7.12 MiB (6.91–7.78 MiB) | 50%: 1.16 ms (1.14–1.22 ms)<br>99%: 8.84 ms (8.39–9.00 ms)<br>C: 14.82 MiB (14.81–14.83 MiB)<br>S: 7.30 MiB (7.29–7.34 MiB) | 50%: 1.13 ms (1.10–1.16 ms)<br>99%: 9.05 ms (9.02–9.21 ms)<br>C: 14.71 MiB (14.60–14.85 MiB)<br>S: 7.17 MiB (7.17–7.19 MiB) | 50%: 1.21 ms (1.18–1.27 ms)<br>99%: 5.42 ms (3.84–8.76 ms)<br>C: 15.00 MiB (14.90–15.01 MiB)<br>S: 7.07 MiB (7.04–7.62 MiB) | 50%: 1.17 ms (1.16–1.21 ms)<br>99%: 16.37 ms (15.58–17.62 ms)<br>C: 6.96 MiB (6.93–6.97 MiB)<br>S: 3.13 MiB (3.13–3.25 MiB) | 50%: 3.08 ms (3.03–3.12 ms)<br>99%: 11.50 ms (9.64–12.21 ms)<br>C: 37.70 MiB (37.45–40.16 MiB)<br>S: 30.24 MiB (29.33–31.19 MiB) |
| 800 B | 1,024 connections | 0% | 50%: 22.70 ms (21.95–24.31 ms)<br>99%: 42.70 ms (41.55–75.24 ms)<br>C: 19.46 MiB (18.15–20.46 MiB)<br>S: 5.02 MiB (4.86–5.08 MiB) | 50%: 21.35 ms (13.79–21.76 ms)<br>99%: 114.12 ms (58.20–120.21 ms)<br>C: 50.53 MiB (50.32–51.90 MiB)<br>S: 35.95 MiB (34.48–36.38 MiB) | 50%: 16.88 ms (10.23–18.16 ms)<br>99%: 117.16 ms (107.80–190.99 ms)<br>C: 50.95 MiB (49.53–51.82 MiB)<br>S: 35.30 MiB (34.23–35.42 MiB) | 50%: 20.42 ms (16.57–21.18 ms)<br>99%: 42.81 ms (40.26–117.52 ms)<br>C: 50.59 MiB (50.52–50.68 MiB)<br>S: 36.27 MiB (34.35–36.39 MiB) | 50%: 12.39 ms (10.59–18.19 ms)<br>99%: 169.10 ms (109.53–187.16 ms)<br>C: 51.48 MiB (51.21–51.66 MiB)<br>S: 35.09 MiB (34.91–36.32 MiB) | 50%: 18.82 ms (18.65–20.18 ms)<br>99%: 107.54 ms (102.72–118.36 ms)<br>C: 22.19 MiB (22.07–22.32 MiB)<br>S: 20.63 MiB (20.12–20.75 MiB) | 50%: 71.29 ms (69.26–71.72 ms)<br>99%: 140.18 ms (109.03–283.41 ms)<br>C: 244.89 MiB (222.95–249.27 MiB)<br>S: 150.57 MiB (149.33–175.30 MiB) |
| 800 B | 64 connections | 1% | 50%: 0.64 ms (0.63–0.66 ms)<br>99%: 7.02 ms (6.99–7.43 ms)<br>C: 11.82 MiB (11.67–11.84 MiB)<br>S: 3.30 MiB (3.23–3.39 MiB) | 50%: 1.03 ms (1.02–1.19 ms)<br>99%: 5.91 ms (5.70–6.11 ms)<br>C: 15.04 MiB (14.72–15.32 MiB)<br>S: 7.23 MiB (7.18–7.94 MiB) | 50%: 1.05 ms (1.03–1.10 ms)<br>99%: 6.06 ms (5.42–6.52 ms)<br>C: 15.16 MiB (14.84–15.44 MiB)<br>S: 7.55 MiB (7.46–7.62 MiB) | 50%: 1.08 ms (0.95–1.16 ms)<br>99%: 5.85 ms (5.10–6.30 ms)<br>C: 14.98 MiB (14.89–15.22 MiB)<br>S: 7.32 MiB (7.31–7.63 MiB) | 50%: 1.02 ms (1.02–1.04 ms)<br>99%: 5.90 ms (5.61–6.11 ms)<br>C: 14.98 MiB (14.95–15.06 MiB)<br>S: 7.50 MiB (7.50–7.53 MiB) | 50%: 1.24 ms (1.03–1.31 ms)<br>99%: 11.33 ms (6.75–11.36 ms)<br>C: 6.79 MiB (6.73–6.89 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.20 ms (3.15–3.29 ms)<br>99%: 13.52 ms (12.98–23.95 ms)<br>C: 40.48 MiB (36.59–41.42 MiB)<br>S: 29.52 MiB (28.62–33.45 MiB) |
| 800 B | 1,024 connections | 1% | 50%: 25.00 ms (19.25–26.59 ms)<br>99%: 82.13 ms (53.30–85.70 ms)<br>C: 20.33 MiB (20.15–20.54 MiB)<br>S: 4.97 MiB (4.71–5.04 MiB) | 50%: 21.11 ms (19.79–22.75 ms)<br>99%: 122.90 ms (90.12–136.39 ms)<br>C: 51.90 MiB (50.38–52.18 MiB)<br>S: 36.71 MiB (36.62–37.41 MiB) | 50%: 30.06 ms (22.12–30.13 ms)<br>99%: 340.38 ms (193.22–355.15 ms)<br>C: 53.13 MiB (52.59–54.25 MiB)<br>S: 38.64 MiB (37.71–39.57 MiB) | 50%: 19.21 ms (17.43–30.82 ms)<br>99%: 195.16 ms (105.69–200.33 ms)<br>C: 51.19 MiB (49.07–53.82 MiB)<br>S: 36.22 MiB (35.62–37.24 MiB) | 50%: 18.11 ms (13.85–21.74 ms)<br>99%: 212.59 ms (201.87–336.37 ms)<br>C: 53.09 MiB (52.64–53.35 MiB)<br>S: 37.80 MiB (37.38–37.97 MiB) | 50%: 23.50 ms (13.31–24.95 ms)<br>99%: 175.31 ms (136.41–264.19 ms)<br>C: 22.32 MiB (22.07–22.32 MiB)<br>S: 19.88 MiB (18.75–20.00 MiB) | 50%: 78.05 ms (75.61–81.64 ms)<br>99%: 365.24 ms (338.86–430.47 ms)<br>C: 245.64 MiB (240.02–252.82 MiB)<br>S: 191.64 MiB (190.14–205.88 MiB) |

### Reading the results

- At 64 B and 1,024 connections without injected loss, default ordinary sends deliver **47.00 MiB/s**, and batch APIs **142.00 MiB/s**. TCP, C KCP and quic-go deliver **53.63 MiB/s, 42.64 MiB/s and 107.73 MiB/s**, respectively.
- For one connection, 800 B messages and 5% loss, default ordinary sends deliver **146.72 MiB/s**. TCP, C KCP and quic-go deliver **1.03 MiB/s, 102.06 MiB/s and 8.07 MiB/s**, respectively. These describe this configured reliable ordered echo workload.
- At 800 B and 1,024 connections without injected loss, the configured policy changes ordinary sends from **372.54 MiB/s to 395.28 MiB/s**, and batch APIs from **413.36 MiB/s to 381.62 MiB/s**.
- At 64 B and 1,024 connections without injected loss, the configured policy changes ordinary loaded median RTT from **16.90 ms to 15.81 ms**, and 99th-percentile RTT from **94.65 ms to 101.42 ms**. Compare throughput, slow-message latency and memory together.

All profiles use reliable ordered traffic. They do not test mixed reliable/unreliable scheduling, abrupt bandwidth changes or congestion fairness. `send-policy` remains optional. This matrix cannot replace mixed-traffic validation and is not a controlled before/after comparison with an older revision.

UDP receive buffers can overflow even without injected loss. Namespace-local UDP error counters and qdisc statistics were recorded for each run. Short workloads, random loss and CPU scheduling produce variation; small differences are not evidence of statistical significance or a universal ranking.

Maximum namespace UDP receive-buffer drops in one throughput run: rust-raknet (`send`): 331,307 datagrams; rust-raknet (batch APIs): 309,627 datagrams; rust-raknet (`send` + `send-policy`): 220,506 datagrams; rust-raknet (batch APIs + `send-policy`): 325,673 datagrams; C KCP: 651,641 datagrams; quic-go: 3,351 datagrams.

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
python3 scripts/test-features.py
```

CI runs this complete feature matrix on Linux, Windows, macOS and Rust 1.85.

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
