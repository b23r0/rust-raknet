# rust-raknet [![GitHub Actions](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml) [![ChatOnDiscord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/ZKtYMvDFN4) [![Crate](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet) [![Crate](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
RakNet Protocol implementation by Rust.

Raknet is a reliable udp transport protocol that is generally used for communication between game clients and servers, and is used by Minecraft Bedrock Edtion for underlying communication.

Raknet protocol supports various reliability options, and has better transmission performance than TCP in unstable network environments. This project is an incomplete implementation of the protocol by reverse engineering.

Requires *Tokio 1.21 or newer* asynchronous runtime support.

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
rust-raknet = "0.14"
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

Measured on **2026-09-30**, comparing this tree with TCP. RakNet uses `ReliableOrdered`; TCP uses length-prefixed records with `TCP_NODELAY`. Sequential request/echo RTT sampling precedes a pipelined burst with concurrent reception.

Environment: Intel Core i7-9700F (8 logical CPUs), Linux x86_64, Rust 1.98.1, Tokio 1.53.1, release builds, four Tokio workers per process. Tests ran in a private user/network namespace: MTU 1,500 bytes, GSO/GRO aggregation limited to one packet, and `tc netem` on its own loopback. Host network settings were unchanged. CPU affinity does not reserve a core exclusively.

### Throughput

**Higher is better.** Values count echoed application payload per direction in MiB/s (1 MiB = 1,048,576 bytes), excluding headers and ACKs. Both protocols have three runs per profile; cells show the median and range. Each run has 100 warmups and 300 RTT samples before the burst.

| Network profile | Payload / burst count | TCP throughput (median, range) | RakNet throughput (median, range) |
| --- | ---: | ---: | ---: |
| 0% loss | 800 B / 200,000 messages | 110.51 MiB/s (97.91–118.01 MiB/s) | 74.76 MiB/s (72.45–77.43 MiB/s) |
| 1% loss | 800 B / 20,000 messages | 9.59 MiB/s (9.29–9.94 MiB/s) | 78.91 MiB/s (63.40–80.28 MiB/s) |
| 5% loss | 800 B / 10,000 messages | 0.78 MiB/s (0.76–1.24 MiB/s) | 65.55 MiB/s (48.09–69.90 MiB/s) |
| 0% loss, small packets | 64 B / 300,000 messages | 9.27 MiB/s (9.25–9.31 MiB/s) | 6.59 MiB/s (6.14–6.59 MiB/s) |
| 0% loss, fragmented messages | 4,096 B / 50,000 messages | 259.88 MiB/s (256.73–264.91 MiB/s) | 102.49 MiB/s (81.58–112.06 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.06 MiB/s (0.96–1.42 MiB/s) | 4.43 MiB/s (4.38–4.53 MiB/s) |

RakNet and TCP each completed **18/18** throughput runs. [Raw throughput measurements](docs/validation-2026-09-30-concurrency/buffer-throughput.jsonl) retain every observation.

### Latency

**Lower is better.** RTT is round-trip latency. Each cell lists p50 / p95 / p99, as medians of run-level percentiles. Clean profiles have five runs per protocol, 10,000 samples after 1,000 warmups. The delayed profile has three runs per protocol, 2,000 samples after 100 warmups. The pinned profiles use separate client/server CPU affinities.

| Network profile | TCP RTT (p50 / p95 / p99) | RakNet RTT (p50 / p95 / p99) |
| --- | ---: | ---: |
| 0% loss, CPU affinity | 16.200 µs / 24.100 µs / 49.500 µs | 25.600 µs / 39.400 µs / 68.400 µs |
| 0% loss, no CPU affinity | 16.900 µs / 25.600 µs / 65.400 µs | 19.500 µs / 32.700 µs / 76.300 µs |
| 1% loss + 5 ms each way, CPU affinity | 10.354 ms / 12.421 ms / 36.637 ms | 10.405 ms / 12.445 ms / 85.924 ms |

These are local single-connection echo measurements. Random loss includes ACKs in both directions and does not reproduce an identical trace between runs. Scheduling and retransmissions affect tail latency. The [validation report](docs/concurrency-and-bedrock-report.md) includes repeat checks, concurrency measurements, limitations, and binary hashes. See the [benchmark instructions](example/test_benchmark/README.md) for isolated reproduction.

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
