# rust-raknet [![GitHub Actions](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml) [![ChatOnDiscord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/ZKtYMvDFN4) [![Crate](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet) [![Crate](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
RakNet Protocol implementation by Rust.

Raknet is a reliable udp transport protocol that is generally used for communication between game clients and servers, and is used by Minecraft Bedrock Edtion for underlying communication.

Raknet protocol supports various reliability options, and has better transmission performance than TCP in unstable network environments. This project is an incomplete implementation of the protocol by reverse engineering.

Requires >= *Tokio 1.x* asynchronous runtime support.

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

# Benchmark

The benchmark compares a TCP echo server with RakNet using `ReliableOrdered` packets. The client first measures request/echo round-trip latency, then sends a pipelined burst while receiving echoes concurrently. TCP uses length-prefixed records. See the [benchmark instructions](example/test_benchmark/README.md) for commands and options.

Command used for these results: 20,000 measured 800-byte packets per run, 200 warmup rounds, and 300 sequential RTT samples; release build; three runs for each protocol and loss profile. The table reports the median of the three run summaries, with the throughput range in parentheses.

| Configured loss | TCP payload MiB/s (median, range) | RakNet payload MiB/s (median, range) | TCP RTT p50 / p95 / p99 | RakNet RTT p50 / p95 / p99 |
| ---: | ---: | ---: | ---: | ---: |
| 0% | 115.90 (102.45–116.26) | 14.33 (2.04–16.42) | 15.6 / 31.6 / 74.8 µs | 222.1 / 409.4 / 487.1 µs |
| 1% | 17.64 (9.30–21.05) | 13.33 (0.99–14.45) | 74.3 / 294 / 4,588 µs | 84 / 279 / 86,390 µs |
| 5% | 1.20 (1.19–1.49) | 13.77 (12.34–13.93) | 112 / 5,051 / 207,972 µs | 94 / 51,133 / 100,194 µs |

Measured on 2026-09-28 on an Intel Core i7-9700F (8 logical CPUs), Linux x86_64, and `rustc 1.98.1`. TCP and RakNet ran in a temporary isolated network namespace with `tc netem` on its loopback; MTU was 1,500 bytes and GSO/GRO were limited to one packet. The host loopback remained unchanged. Per-run qdisc counters confirmed approximately 1% and 5% drops. The 5% profile yielded 11.5× higher median RakNet payload throughput, while 0% favored TCP throughput; 1% results were close and varied between runs. These results describe this local echo workload, not Internet performance. MiB/s counts application payload in one direction and excludes protocol headers and acknowledgements. RTT values are medians of the three run-level percentiles; loss-induced tails are visible in p99.

Benchmark source: [example/test_benchmark/src/main.rs](example/test_benchmark/src/main.rs).

## Contributing

Contributions are welcome! You can help by reporting bugs, suggesting features, improving documentation, adding examples, or submitting code changes.

### Send a contribution

1. For a larger change, open an issue first so we can agree on the approach. Bug reports are most helpful with steps to reproduce and expected behavior.
2. Fork the repository and create a focused branch for your change.
3. Format the code and run the same build and test checks used by CI:

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
