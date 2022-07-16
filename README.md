# rust-raknet [![Build Status](https://img.shields.io/github/workflow/status/b23r0/rust-raknet/Rust)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml) [![ChatOnDiscord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/ZKtYMvDFN4) [![Crate](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet) [![Crate](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/) 
RakNet Protocol implementation by Rust.

Raknet is a reliable udp transport protocol that is generally used for communication between game clients and servers, and is used by Minecraft Bedrock Edtion for underlying communication.

Raknet protocol supports various reliability options, and has better transmission performance than TCP in unstable network environments. This project is an incomplete implementation of the protocol by reverse engineering.

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
* Compatible with Minecraft 1.18.x

# Get Started

```toml
# Cargo.toml
[dependencies]
rust-raknet = "*"
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
        let mut socket = listener.accept().await.unwrap();
        let buf = socket.recv().await.unwrap();
        if buf[0] == 0xfe{
            //do something
        }
    }
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
}
```

# Benchmark

Use Tcp to compare with this project. Set the server packet loss rate to 50%, the client connects to the server, and the server sends an 800-byte packet every 30ms, a total of 100 times. The client counts the delay time of each received data, and calculates the average time of receiving 100 times. The following results are obtained.

Test code: https://github.com/b23r0/rust-raknet/blob/main/example/test_benchmark/src/main.rs

Result:

![image]( https://github.com/b23r0/rust-raknet/blob/main/images/benchmark20220612.jpg)

(June 12, 2022)

In the network environment with high packet loss rate, this project can reduce the delay time by about 50% compared with TCP.

# Contribution

Options :

* Report a BUG
* Submit an ISSUE about suggestion
* Submit a improved PR
* Add an example of using rust-raknet
* Supplement the documentation on using rust-raknet

This project exists thanks to all the people who contribute. 
