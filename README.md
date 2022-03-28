# rust-raknet [![Build Status](https://img.shields.io/github/workflow/status/b23r0/rust-raknet/Rust)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml) [![ChatOnDiscord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/ZKtYMvDFN4)
RakNet Protocol implementation by Rust.

Raknet is a reliable udp transport protocol that is often used for communication between game clients and servers. This project is an incomplete implementation of the protocol.

_This project is not affiliated with Jenkins Software LLC nor RakNet._

# Features

* Async
* MIT License
* Pure Rust implementation
* Compatible with Minecraft 1.18.x

# Get Started

```toml
# Cargo.toml
[dependencies]
rust-raknet = "0.1.0"
```

# Reliability

- [x] unreliable
- [x] unreliable sequenced
- [x] reliable
- [x] reliable ordered
- [x] reliable sequenced
- [ ] unreliable (+ ACK receipt)
- [ ] reliable (+ ACK receipt)
- [ ] reliable ordered (+ ACK receipt)

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
//clients

async fn connect(){
    let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    let buf = socket.recv().await.unwrap();
    if buf[0] == 0xfe{
        //do something
    }
    socket.close();
}
```

# Contribution

If you want to develop with me, you can contact me via discord or email.
