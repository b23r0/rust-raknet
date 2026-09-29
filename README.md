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

This example covers server-list status discovery. Game login and gameplay packets are outside the crate's current APIs.

## Bedrock transport compatibility

The `example/proxy` program is a RakNet/UDP proxy. It works with Bedrock servers configured as `transport=raknet`; it cannot accept the TCP/WebRTC transport that recent Bedrock Dedicated Server versions use by default. The RakNet proxy negotiates the upstream RakNet version accepted from its client.

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

Application sends wait for the connected handshake and apply asynchronous backpressure when the send queue is full. Reliable delivery keeps a 64-datagram flight window. A burst normally queues up to 256 KiB including frame overhead; a single larger `ReliableOrdered` message can be admitted when the queue is empty, up to a 64 MiB queue budget that charges payload bytes plus 128 bytes per frame. This accounting limit is not a process RSS limit.

Receive reordering is limited to 65,536 reliable indexes and 64 MiB of ordered payload. Fragment reassembly allows at most 1,024 concurrent groups, 65,536 fragments per group, and 64 MiB including frame overhead. An incomplete group that makes no progress for 60 seconds closes the connection. Invalid or excessive receive state is disconnected rather than acknowledged and silently discarded. Applications should consume incoming messages concurrently with sustained sends.

`NetherNetProxy` limits active signaling connections to 1,024 by default; use `with_connection_limit(NonZeroUsize)` to choose another limit. Upstream connection attempts time out after 10 seconds. Cancelling `run()` also cancels its active forwarding tasks.

# Benchmark

Measured on **2026-09-30**, comparing the RakNet implementation in this tree with TCP. RakNet uses `ReliableOrdered`; TCP uses length-prefixed records with `TCP_NODELAY`. The client measures sequential request/echo RTT, then sends a pipelined burst while receiving echoes concurrently.

Environment: Intel Core i7-9700F (8 logical CPUs), Linux x86_64, Rust 1.98.1, Tokio 1.53.1, release builds. Network tests ran in a private user/network namespace with `tc netem` applied only to its loopback. MTU was 1,500 bytes, with GSO/GRO aggregation limited to one packet. The host network configuration was unchanged.

### Throughput

**Higher is better.** Throughput is measured in MiB/s (1 MiB = 1,048,576 bytes).

MiB/s counts echoed application payload **per direction**, excluding headers and ACKs. Each run used 100 warmup rounds and 300 RTT samples before its measured burst. RakNet values are medians of three successful runs, with ranges shown in parentheses. **TCP has one control run per profile**, so its values are single observations.

| Network profile | Payload size / burst count | TCP throughput (single run) | RakNet throughput (median, range) |
| --- | ---: | ---: | ---: |
| 0% loss | 800 B / 20,000 messages | 114.83 MiB/s | **75.28 MiB/s** (74.27–75.74 MiB/s) |
| 1% loss | 800 B / 20,000 messages | 19.89 MiB/s | **74.94 MiB/s** (71.74–78.15 MiB/s) |
| 5% loss | 800 B / 20,000 messages | 1.07 MiB/s | **68.86 MiB/s** (47.49–73.01 MiB/s) |
| 0% loss, small packets | 64 B / 30,000 messages | 10.10 MiB/s | **4.02 MiB/s** (3.91–5.40 MiB/s) |
| 0% loss, fragmented messages | 4,096 B / 5,000 messages | 262.76 MiB/s | **86.64 MiB/s** (83.17–93.61 MiB/s) |
| 1% loss + 5 ms each way | 800 B / 3,000 messages | 1.04 MiB/s | **4.46 MiB/s** (4.26–4.56 MiB/s) |

RakNet completed **18/18** throughput runs, and TCP completed all six control runs. See the [raw results](docs/validation-2026-09-30/comparison.jsonl) for the individual measurements.

### Latency

**Lower is better.** RTT is round-trip latency; µs means microseconds and ms means milliseconds (1 ms = 1,000 µs). Each cell lists p50 / p95 / p99 in that order.

The separate focused checks use fixed client/server CPU affinities. The clean check has five RakNet runs of 10,000 measured RTTs after 1,000 warmups; the delayed 1% loss check has three runs of 2,000 RTTs after 100 warmups. RakNet entries below are medians of the **run-level percentiles**; each TCP entry is one control run with the same sample count per run.

| Network profile | TCP RTT (single run) | RakNet RTT |
| --- | ---: | ---: |
| 0% loss | 16.2 µs / 30.7 µs / 82.0 µs | 25.6 µs / 43.3 µs / 128.1 µs |
| 1% loss + 5 ms each way | 10.361 ms / 13.332 ms / 221.831 ms | 10.431 ms / 12.902 ms / 77.670 ms |

Random loss and scheduling affect tail latencies; the TCP measurements are single-run controls.

These results describe a local, single-connection echo workload. Random loss applies in both directions, including ACKs, and is not an identical packet-loss trace across runs. The [validation report](docs/optimization-report.md) includes per-run data, CPU/memory measurements, latency ranges, binary hashes, and compatibility limits. See the [benchmark instructions](example/test_benchmark/README.md) for isolated reproduction and the [benchmark source](example/test_benchmark/src/main.rs) for implementation details.

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
