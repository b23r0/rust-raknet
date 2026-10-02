<div align="center">

# rust-raknet

[English](README.md) | **简体中文**

**高性能 RakNet 协议的 Rust 实现。**

[![Build](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet)
[![Documentation](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2)](https://discord.gg/ZKtYMvDFN4)

[快速开始](#快速开始) · [基岩版反向代理](#minecraft-基岩版) · [性能测试](#性能测试) · [参与贡献](#参与贡献)

</div>

`rust-raknet` 为 Rust 应用提供基于 UDP 的可靠消息传输。它基于 Tokio，实现了 RakNet 的握手、确认、重传、排序和分片机制。你可以根据业务需要，选择不同的消息交付方式。

- 提供客户端和监听器接口，支持 RakNet 的五种可靠模式。
- 有界发送队列、背压和选择性重传。
- 可配置 MTU、连接接收队列和 UDP 接收缓冲区。
- Linux 下可选接收 socket 分片，适用于繁忙的服务端。
- 纯 Rust 实现，采用 MIT 协议，支持 Linux、Windows、macOS 和 BSD。

**环境要求：** Rust 1.85+、Tokio 1.38+。

## 快速开始

```toml
[dependencies]
rust-raknet = "0.16.0"
tokio = { version = "1.38", features = ["full"] }
```

### 回显服务端

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

### 客户端

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

应用消息不能为空，且必须以 `0xfe` 开头。持续发送大量消息时，也要持续接收，让对端能够继续处理。

| 可靠模式 | 交付方式 | 排序方式 |
| --- | --- | --- |
| `Unreliable` | 尽力交付 | 无 |
| `UnreliableSequenced` | 尽力交付 | 丢弃过时消息 |
| `Reliable` | 重传直到收到确认 | 无 |
| `ReliableOrdered` | 重传直到收到确认 | 按通道有序交付 |
| `ReliableSequenced` | 可靠传输，带消息序列 | 丢弃过时消息 |

`send_with_order_channel`、`flush`、服务器发现和监听器配置等接口，见 [API 文档](https://docs.rs/rust-raknet/latest/rust_raknet/)。

### 转发拥有所有权的缓冲区

通过 `recv_bytes()` 和 `send_bytes()`，代理可以与发送队列、重传队列共享不可变的消息缓冲区，避免再次复制应用数据：

```rust
use rust_raknet::{RaknetSocket, Reliability, error::Result};

async fn forward(source: &RaknetSocket, destination: &RaknetSocket) -> Result<()> {
    loop {
        let payload = source.recv_bytes().await?;
        destination.send_bytes(payload, Reliability::ReliableOrdered).await?;
    }
}
```

使用 `send_bytes_with_order_channel()` 可以指定排序通道。原有的切片发送接口和返回 `Vec<u8>` 的接收接口仍然可用，两种路径遵守相同的队列限制、分片规则和交付保证。

### 批量处理已准备好的消息

`send_batch` 和 `send_bytes_batch` 可以把较小的 `ReliableOrdered` 消息合并到标准 RakNet 帧集合中，不会等待更多消息来凑包。其他模式和需要分片的消息仍使用独立数据报；如果两条消息无法放进一个包，也会沿用普通发送路径。每个数据报最多合并八条消息，以控制单次丢包的影响。收到丢包反馈或发生重传超时后，该连接在剩余生命周期内改为逐包发送。每条消息保留自己的交付索引和排序通道，一个数据报的 ACK 会确认其中所有可靠消息。

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

批量接收会清空并复用传入的 vector，一次最多返回 64 条消息。需要指定排序通道时，使用 `send_batch_with_order_channel` 或 `send_bytes_batch_with_order_channel`。普通 `send` 调用仍立即尝试发送。批量消息全部校验通过后才会入队；取消一次发送，可能留下已经入队、等待交付的部分消息。

### 示例目录

所有示例都放在 `examples/` 下。单文件程序使用 `cargo run --example NAME` 运行。
`proxy`、`bedrock_ping` 和 `test_benchmark` 子目录是独立的 Cargo 项目，
使用 `cargo run --manifest-path examples/PROJECT/Cargo.toml` 运行。

## Minecraft 基岩版

本库按字节传输基岩版数据包，不实现 Xbox 登录，也不解析游戏协议。[反向代理示例](examples/proxy) 用于转发 RakNet UDP 游戏流量，后端服务器需要配置 `transport=raknet`。

### 在基岩版 26.52 中启用 RakNet

截至 2026 年 10 月 2 日，基岩版最新稳定版本为 [26.52](https://feedback.minecraft.net/hc/en-us/articles/49175370527501-Minecraft-Bedrock-Edition-26-52-Hotfix-Changelog)。已测试的环境使用 Windows 1.26.52 客户端和基岩版专用服务端 1.26.52.3，游戏协议版本为 2193，RakNet 协议版本为 11。通过 `rust-raknet` 0.16.0 的 RakNet UDP 代理，加入世界、走动、放置和破坏方块，以及服务器列表 MOTD 均正常。

从 [26.50](https://feedback.minecraft.net/hc/en-us/articles/48826825649933-Minecraft-Bedrock-Edition-26-50-Changelog-Wilderness-Bound) 开始，专用服务端默认使用 NetherNet。在已测试的 26.52 环境中，先停止服务端，再修改 `server.properties`：

```properties
transport=raknet
server-port=19142
server-portv6=19143
enable-lan-visibility=false
```

保存后重启专用服务端。`transport=raknet` 选择传统 UDP 传输；关闭局域网可见性，可以避免使用自定义端口时额外监听默认端口。测试时应将后端和代理放在同一个隔离网络中，下方代理命令会转发到后端的 19142 端口。

调用 `listen()` 前，需要为代理监听器配置与后端匹配的基岩版服务器公告。在 `examples/proxy/src/main.rs` 的 `listener.listen().await` 前加入：

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

实际部署时，请使用后端对应的协议版本、游戏版本和游戏模式。监听器默认公告使用较旧的基岩版本信息；即使 UDP 握手成功，错误的公告也可能导致新客户端无法加入。

### 基岩版对 RakNet 的弃用

Mojang 正在把基岩版网络传输迁移到 NetherNet。[26.60.22/23 预览版更新说明](https://feedback.minecraft.net/hc/en-us/articles/48740748263565-Minecraft-Beta-Preview-26-60-22-23) 将 RakNet 标记为弃用，并把专用服务端使用其他传输方式时的警告升级为错误。我们的 26.52 服务端也打印了仅支持 NetherNet 的错误信息，但 RakNet 连接和游戏操作测试仍然成功。

上述兼容性结果仅覆盖实际测试过的 26.52 构建，不能据此认定支持 26.60 或后续版本。引用的预览版说明也没有给出最终移除日期。依赖 RakNet 的部署在升级前应查看更新说明并重新测试。`rust-raknet` 实现的是 RakNet UDP；NetherNet 使用 WebRTC，需要不同的传输实现。

### RakNet 反向代理

```sh
cargo run --release --manifest-path examples/proxy/Cargo.toml -- \
  -l 127.0.0.1:19144 -r 127.0.0.1:19142
```

本地测试时，在客户端服务器列表中添加 `127.0.0.1:19144`。从其他机器连接时，使用可访问的代理地址和前端端口。客户端必须连接代理入口，游戏流量才会经过 `rust-raknet`。

代理连接上游时会沿用客户端的 RakNet 版本。两个方向并发转发，上游握手超时为 10 s。Linux 下可添加 `--socket-shards 4`，启用四个接收 socket；默认使用一个。当多个前端分片共享单个上游 socket 时，也可能比单分片更慢。添加 `--batch-messages` 可使用显式批量接口转发已准备好的消息，该选项默认关闭，不会等待凑包。

### 服务器发现

```sh
cargo run --manifest-path examples/bedrock_ping/Cargo.toml -- play.example.com:19132
```

该示例发送未连接的 RakNet ping，并打印服务器名称、游戏版本、玩家数量和 MOTD。

## 配置

<details>
<summary><b>MTU 协商</b></summary>

默认名义 MTU 为 1,400 B，两端都可以指定其他上限：

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

允许范围为 61–1,492 B，协商结果取双方上限的较小值，可以通过 `mtu()` 获取。名义 MTU 为 IPv4/UDP 包头预留了 28 B；使用 IPv6 或隧道时，还需要考虑额外开销。这不是路径 MTU 发现机制。

</details>

<details>
<summary><b>Linux 接收 socket 分片</b></summary>

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

`SO_REUSEPORT` 会把不同对端的流量分配到多个 socket。它们共享 GUID、MOTD 和连接接收队列，并作为同一个监听器关闭。分片数量在关闭前保持不变，运行时也需要足够的工作线程。默认使用一个 socket，调整前请先测试实际负载。

持续流量场景下，Linux 监听器还可以在调用 `listen()` 前启用 `listener.with_receive_batching(true)`。一次系统调用最多接收 16 个已经就绪的数据报，不会等待凑满批次。该设置适用于所有分片，默认关闭，因为批量接收可能增加稀疏流量的处理延迟。启用前请对照实际负载测试，分片回显示例提供 `--receive-batching` 参数。

大量连接处于空闲状态时，可以启用 `listener.with_idle_maintenance(true)`：在 500 ms 内没有收到流量且队列为空后，暂停周期维护，有新任务时再恢复。已连接的客户端可以通过 `socket.set_idle_maintenance(true)` 启用，并在运行时关闭。该选项默认也关闭；它能减少空闲 CPU 开销，但可能改变负载下的延迟。启用前应分别测试繁忙和空闲场景，分片回显示例提供 `--idle-maintenance` 参数。

</details>

<details>
<summary><b>队列、接收限制和连接关闭</b></summary>

- 发送会等待连接握手完成；发送队列满时，会等待队列腾出空间。最多允许 64 个可靠帧在途，包括分片。普通突发流量的队列上限为 256 KiB，含帧开销。较大的 `ReliableOrdered` 消息可以进入空队列，但受 64 MiB 预算和 65,536 个分片的限制。预算按有效载荷加每帧 128 B 计算，不是进程总内存限制。
- 接收重排最多允许 65,536 个可靠索引和 64 MiB 有序消息数据。分片重组最多允许 1,024 组，每组 65,536 个分片，总预算为 64 MiB，含帧开销。某组停滞 60 s 会关闭连接；接收状态无效或超限时，也会关闭连接，而不是静默丢弃已经确认的数据。
- `bind()` 会申请 2 MiB UDP 接收缓冲区，操作系统可能限制实际大小。可以通过 `bind_with_receive_buffer_size()` 指定其他大小；`from_std()` 保留传入 socket 的缓冲区设置。
- 默认连接接收队列长度为 128，可以在 `listen()` 前通过 `with_accept_backlog(NonZeroUsize)` 调整。队列满时，新的离线握手会延后到下一次重试；这不是活跃会话数量上限。

</details>

## 性能测试

以下对比的是 **rust-raknet 的具体实现**，分别使用普通发送接口和显式批量接口，与 TCP、官方 C KCP 核心和 quic-go 对比。这些数据不代表 RakNet 协议本身的通用性能。

**测试日期：2026 年 10 月 2 日。** 环境为 Intel Core i7-9700F（8 个逻辑 CPU）、Linux x86_64、Rust 1.98.1、Tokio 1.53.1、GCC 13.3.0、Go 1.27.1，均使用优化构建。所有进程都在任务专属副本中运行，使用独立缓存和独立回环网络命名空间。回环 MTU 为 1,500 B，GSO/GRO 限制为单包，netem 队列上限为 100,000 个包，进程使用 `nice 10`。未修改主机网络设置。

### 对比实现与接口模式

| 表格列 | GitHub 仓库 | 测试版本 / 提交 |
| --- | --- | --- |
| TCP | [TCP 回显驱动](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark), [Tokio](https://github.com/tokio-rs/tokio) | 驱动 0.1.0； Tokio 1.53.1; Linux TCP 协议栈 7.0.11-76070011-generic |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 0.16.0 工作区，基于 `a957256`，包含尚未提交的性能改动 |
| rust-raknet（批量接口） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 与 `send` 列使用同一份库构建 |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | 固定提交 `b1a7a21`，未修改上游 C 核心 |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

- **`send`**：两端均使用普通 `send` / `recv`，不调用批量接口。
- **批量接口**：单连接客户端使用 `send_batch`，并发客户端使用 `send_bytes_batch`；回显服务端使用 `recv_bytes_batch` 和 `send_bytes_batch`。每次调用最多提交 16 条已准备好的消息，两端都不等待凑满批次。

并发批量客户端会为每批消息构造拥有所有权的缓冲区，普通客户端则复用借用的消息模板。因此，对比包含缓冲区构造、发送接口和接收队列批量取出的开销，不能单独用于衡量合包的成本或收益。

每次测试都会启动新的服务端并建立新连接。两列 rust-raknet 均使用 `ReliableOrdered`、正常的 64 帧可靠在途上限和正常重试设置。可选的 UDP 批量接收和空闲维护均关闭。消息合并与 UDP 系统调用的批量发送是不同机制，后者在两种接口模式中都可以使用。

在 MTU 范围内，一个 UDP 数据报最多合并**八条**不需要分片的消息。测试使用的 MTU 放不下两条 800 B 消息；4,096 B 消息需要分片，也不会合包。这些批量列衡量的是批量接口和批量接收路径，而不是消息合并。收到匹配的 NACK 或发生可靠消息重传超时后，该连接会永久停止合包，包括预热阶段发生的反馈。因此，**调用批量接口不等于每轮测试都实际合并了消息**。

TCP 使用 `TCP_NODELAY`、四字节小端长度前缀、整条记录写入，并复用缓冲区。C KCP 使用消息模式，发送/接收分段窗口为 64/128，配置 `nodelay(1, 10, 2, 1)`，立即写入和确认，不使用 FEC 或加密。QUIC 每条连接使用一个双向可靠有序流，记录格式与 TCP 相同，保留加密、拥塞控制和流量控制；rust-raknet 和 C KCP 不提供同样的功能。

吞吐量按校验通过的回显应用数据计算，统计**单方向**，不包含包头和 ACK，**越高越好**。表格展示三轮测试的中位数，五列实现的运行顺序轮换。1 MiB = 1,048,576 B。每轮使用独立随机丢包，数据和 ACK 的两个方向均受影响。

### 单连接吞吐量

所有客户端最多允许 64 条应用消息等待回显。每轮在吞吐测试前执行 100 次预热和 300 次串行 RTT 采样。Rust 和 Go 使用四个工作线程，C KCP 每个进程使用一个事件循环。吞吐测试不绑定 CPU，因此这些结果不是相同 CPU 成本下的效率比较。

| 网络条件 | 消息大小 / 突发数量 | TCP | rust-raknet (`send`) | rust-raknet（批量接口） | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- |
| 无注入丢包 | 800 B / 200,000 条消息 | 149.64 MiB/s | 188.84 MiB/s | 194.79 MiB/s | 150.33 MiB/s | 114.28 MiB/s |
| 1% 丢包 | 800 B / 50,000 条消息 | 17.87 MiB/s | 148.62 MiB/s | 150.42 MiB/s | 130.82 MiB/s | 73.61 MiB/s |
| 5% 丢包 | 800 B / 50,000 条消息 | 1.10 MiB/s | 138.58 MiB/s | 138.57 MiB/s | 104.44 MiB/s | 9.24 MiB/s |
| 无注入丢包 | 64 B / 300,000 条消息 | 13.45 MiB/s | 16.49 MiB/s | 19.81 MiB/s | 12.45 MiB/s | 19.07 MiB/s |
| 1% 丢包 | 64 B / 100,000 条消息 | 3.25 MiB/s | 14.82 MiB/s | 15.53 MiB/s | 11.23 MiB/s | 15.70 MiB/s |
| 5% 丢包 | 64 B / 100,000 条消息 | 0.12 MiB/s | 14.81 MiB/s | 11.98 MiB/s | 10.10 MiB/s | 3.46 MiB/s |
| 无注入丢包 | 4,096 B / 50,000 条消息 | 343.72 MiB/s | 313.39 MiB/s | 316.06 MiB/s | 255.39 MiB/s | 219.47 MiB/s |
| 1% 丢包 + 单向 5 ms 延迟 | 800 B / 3,000 条消息 | 1.04 MiB/s | 3.02 MiB/s | 2.96 MiB/s | 2.97 MiB/s | 1.57 MiB/s |

<details>
<summary>单连接吞吐量范围</summary>

| 网络条件 | 消息大小 / 突发数量 | TCP | rust-raknet (`send`) | rust-raknet（批量接口） | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- |
| 无注入丢包 | 800 B / 200,000 条消息 | 149.64 MiB/s (149.44–157.64 MiB/s) | 188.84 MiB/s (186.11–189.02 MiB/s) | 194.79 MiB/s (191.36–195.96 MiB/s) | 150.33 MiB/s (146.79–153.49 MiB/s) | 114.28 MiB/s (85.89–115.88 MiB/s) |
| 1% 丢包 | 800 B / 50,000 条消息 | 17.87 MiB/s (14.64–25.13 MiB/s) | 148.62 MiB/s (148.36–150.92 MiB/s) | 150.42 MiB/s (125.54–166.60 MiB/s) | 130.82 MiB/s (125.84–149.80 MiB/s) | 73.61 MiB/s (64.52–87.73 MiB/s) |
| 5% 丢包 | 800 B / 50,000 条消息 | 1.10 MiB/s (1.02–1.12 MiB/s) | 138.58 MiB/s (73.17–168.10 MiB/s) | 138.57 MiB/s (85.28–143.16 MiB/s) | 104.44 MiB/s (95.78–122.29 MiB/s) | 9.24 MiB/s (8.35–10.51 MiB/s) |
| 无注入丢包 | 64 B / 300,000 条消息 | 13.45 MiB/s (13.18–14.03 MiB/s) | 16.49 MiB/s (16.47–16.58 MiB/s) | 19.81 MiB/s (16.45–35.17 MiB/s) | 12.45 MiB/s (12.39–12.70 MiB/s) | 19.07 MiB/s (13.44–31.32 MiB/s) |
| 1% 丢包 | 64 B / 100,000 条消息 | 3.25 MiB/s (2.81–3.69 MiB/s) | 14.82 MiB/s (14.52–16.11 MiB/s) | 15.53 MiB/s (14.44–16.70 MiB/s) | 11.23 MiB/s (10.53–11.92 MiB/s) | 15.70 MiB/s (13.10–21.80 MiB/s) |
| 5% 丢包 | 64 B / 100,000 条消息 | 0.12 MiB/s (0.12–0.14 MiB/s) | 14.81 MiB/s (10.86–15.24 MiB/s) | 11.98 MiB/s (6.26–13.47 MiB/s) | 10.10 MiB/s (9.99–10.23 MiB/s) | 3.46 MiB/s (2.88–3.82 MiB/s) |
| 无注入丢包 | 4,096 B / 50,000 条消息 | 343.72 MiB/s (308.23–348.77 MiB/s) | 313.39 MiB/s (312.68–314.14 MiB/s) | 316.06 MiB/s (310.03–316.51 MiB/s) | 255.39 MiB/s (249.93–259.25 MiB/s) | 219.47 MiB/s (218.10–227.73 MiB/s) |
| 1% 丢包 + 单向 5 ms 延迟 | 800 B / 3,000 条消息 | 1.04 MiB/s (0.98–1.16 MiB/s) | 3.02 MiB/s (2.84–3.27 MiB/s) | 2.96 MiB/s (2.94–3.01 MiB/s) | 2.97 MiB/s (2.95–3.04 MiB/s) | 1.57 MiB/s (1.55–1.64 MiB/s) |

</details>

单连接测试中，各 UDP 实现的包大小预算一致：rust-raknet 名义 MTU 为 1,428 B，包含 28 B IPv4/UDP 开销；C KCP 和 quic-go 的 UDP 数据报预算为 1,400 B。QUIC 的路径 MTU 发现已关闭。每条 4,096 B 消息在 rust-raknet 和 C KCP 中都分成三片；本库默认名义 MTU 仍为 1,400 B。

### 稀疏请求的往返延迟

**越低越好。** 下表取五轮测试中各轮 RTT 分位值的中位数；每轮包含 1,000 次预热和 10,000 次串行采样，消息大小为 800 B，不注入丢包。客户端和服务端分别绑定不同 CPU；每个 Rust/Go 进程在指定 CPU 上运行四个工作线程，C KCP 使用一个事件循环。CPU 绑定不代表独占 CPU。

| 实现 / 接口 | 中位 RTT | 95% 分位 RTT | 99% 分位 RTT |
| --- | --- | --- | --- |
| TCP | 12.8 µs | 20.6 µs | 37.9 µs |
| rust-raknet (`send`) | 17.6 µs | 22.7 µs | 47.9 µs |
| rust-raknet（批量接口） | 17.4 µs | 22.2 µs | 47.0 µs |
| C KCP | 10.4 µs | 15.9 µs | 33.3 µs |
| quic-go | 61.9 µs | 94.6 µs | 129.6 µs |

这组稀疏采样中，批量客户端每次只向 `send_batch` 提交**一条**消息，批量服务端也不会等待更多消息后才回复，因此没有实际合包。结果衡量的是稀疏请求的接口路径，而非突发流量的交付延迟。并发吞吐测试也会输出突发发送前的 RTT，但下方负载延迟表不使用这些数据。

### 高并发吞吐量

每轮校验 **1,048,576 条有序回显**，每条连接的应用窗口为 16 条消息。每条回显都会校验连接 ID、消息 ID 和有效载荷。所有连接先各自完成 20 次串行采样，再通过统一屏障开始吞吐测试；建连和采样时间不计入吞吐计时，所有连接保持打开，直到全部突发发送完成。

所有实现均使用四个工作线程，以及四个启用 `SO_REUSEPORT` 的服务端监听器或接收 socket。服务端绑定四个 CPU，客户端绑定另外四个。这些是模拟的传输会话，不是已登录的 Minecraft 玩家。

| 消息大小 | 连接数 | 注入丢包率 | TCP | rust-raknet (`send`) | rust-raknet（批量接口） | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 条连接 | 0% | 634.89 MiB/s | 573.33 MiB/s | 574.74 MiB/s | 390.10 MiB/s | 231.72 MiB/s |
| 800 B | 256 条连接 | 0% | 602.00 MiB/s | 559.84 MiB/s | 580.24 MiB/s | 412.41 MiB/s | 189.96 MiB/s |
| 800 B | 1,024 条连接 | 0% | 577.50 MiB/s | 495.19 MiB/s | 456.42 MiB/s | 351.83 MiB/s | 161.41 MiB/s |
| 800 B | 2,048 条连接 | 0% | 560.90 MiB/s | 437.78 MiB/s | 317.01 MiB/s | 313.08 MiB/s | 127.58 MiB/s |
| 800 B | 64 条连接 | 1% | 404.40 MiB/s | 486.37 MiB/s | 537.64 MiB/s | 342.39 MiB/s | 190.13 MiB/s |
| 800 B | 256 条连接 | 1% | 492.03 MiB/s | 446.77 MiB/s | 476.44 MiB/s | 400.05 MiB/s | 178.44 MiB/s |
| 800 B | 1,024 条连接 | 1% | 490.61 MiB/s | 439.75 MiB/s | 454.97 MiB/s | 362.80 MiB/s | 72.49 MiB/s |
| 800 B | 2,048 条连接 | 1% | 480.96 MiB/s | 346.41 MiB/s | 353.56 MiB/s | 336.77 MiB/s | 38.47 MiB/s |
| 64 B | 64 条连接 | 0% | 68.60 MiB/s | 47.59 MiB/s | 129.31 MiB/s | 32.87 MiB/s | 135.46 MiB/s |
| 64 B | 1,024 条连接 | 0% | 55.39 MiB/s | 49.03 MiB/s | 148.73 MiB/s | 43.48 MiB/s | 110.25 MiB/s |
| 64 B | 64 条连接 | 1% | 46.47 MiB/s | 44.86 MiB/s | 46.56 MiB/s | 32.54 MiB/s | 66.16 MiB/s |
| 64 B | 1,024 条连接 | 1% | 47.38 MiB/s | 42.28 MiB/s | 46.91 MiB/s | 43.08 MiB/s | 80.70 MiB/s |

<details>
<summary>高并发吞吐量范围</summary>

| 消息大小 | 连接数 | 注入丢包率 | TCP | rust-raknet (`send`) | rust-raknet（批量接口） | C KCP | quic-go |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 条连接 | 0% | 634.89 MiB/s (604.49–691.88 MiB/s) | 573.33 MiB/s (549.30–578.24 MiB/s) | 574.74 MiB/s (573.26–583.02 MiB/s) | 390.10 MiB/s (331.71–391.23 MiB/s) | 231.72 MiB/s (224.07–232.60 MiB/s) |
| 800 B | 256 条连接 | 0% | 602.00 MiB/s (599.13–632.16 MiB/s) | 559.84 MiB/s (515.24–567.53 MiB/s) | 580.24 MiB/s (553.57–585.65 MiB/s) | 412.41 MiB/s (410.23–418.18 MiB/s) | 189.96 MiB/s (189.17–194.65 MiB/s) |
| 800 B | 1,024 条连接 | 0% | 577.50 MiB/s (455.40–585.45 MiB/s) | 495.19 MiB/s (490.50–506.24 MiB/s) | 456.42 MiB/s (406.09–509.81 MiB/s) | 351.83 MiB/s (302.39–407.08 MiB/s) | 161.41 MiB/s (148.02–168.02 MiB/s) |
| 800 B | 2,048 条连接 | 0% | 560.90 MiB/s (485.08–571.37 MiB/s) | 437.78 MiB/s (397.60–440.29 MiB/s) | 317.01 MiB/s (295.58–443.27 MiB/s) | 313.08 MiB/s (295.83–346.13 MiB/s) | 127.58 MiB/s (81.90–137.95 MiB/s) |
| 800 B | 64 条连接 | 1% | 404.40 MiB/s (375.90–484.43 MiB/s) | 486.37 MiB/s (449.86–503.03 MiB/s) | 537.64 MiB/s (519.54–558.08 MiB/s) | 342.39 MiB/s (325.80–373.99 MiB/s) | 190.13 MiB/s (185.66–197.30 MiB/s) |
| 800 B | 256 条连接 | 1% | 492.03 MiB/s (490.61–544.03 MiB/s) | 446.77 MiB/s (374.17–495.70 MiB/s) | 476.44 MiB/s (472.61–514.43 MiB/s) | 400.05 MiB/s (390.22–420.57 MiB/s) | 178.44 MiB/s (177.19–178.79 MiB/s) |
| 800 B | 1,024 条连接 | 1% | 490.61 MiB/s (325.94–510.10 MiB/s) | 439.75 MiB/s (430.81–468.65 MiB/s) | 454.97 MiB/s (431.67–458.71 MiB/s) | 362.80 MiB/s (358.80–382.40 MiB/s) | 72.49 MiB/s (60.68–111.68 MiB/s) |
| 800 B | 2,048 条连接 | 1% | 480.96 MiB/s (437.30–493.70 MiB/s) | 346.41 MiB/s (336.19–367.02 MiB/s) | 353.56 MiB/s (328.69–389.72 MiB/s) | 336.77 MiB/s (319.42–346.90 MiB/s) | 38.47 MiB/s (33.38–46.39 MiB/s) |
| 64 B | 64 条连接 | 0% | 68.60 MiB/s (67.87–73.23 MiB/s) | 47.59 MiB/s (47.32–47.99 MiB/s) | 129.31 MiB/s (113.26–157.07 MiB/s) | 32.87 MiB/s (28.76–35.58 MiB/s) | 135.46 MiB/s (133.70–137.00 MiB/s) |
| 64 B | 1,024 条连接 | 0% | 55.39 MiB/s (54.94–55.73 MiB/s) | 49.03 MiB/s (47.57–51.86 MiB/s) | 148.73 MiB/s (124.90–149.36 MiB/s) | 43.48 MiB/s (40.99–43.67 MiB/s) | 110.25 MiB/s (108.30–111.87 MiB/s) |
| 64 B | 64 条连接 | 1% | 46.47 MiB/s (43.87–54.31 MiB/s) | 44.86 MiB/s (44.67–46.52 MiB/s) | 46.56 MiB/s (46.21–47.98 MiB/s) | 32.54 MiB/s (31.50–34.95 MiB/s) | 66.16 MiB/s (63.31–66.84 MiB/s) |
| 64 B | 1,024 条连接 | 1% | 47.38 MiB/s (44.98–51.74 MiB/s) | 42.28 MiB/s (42.08–42.58 MiB/s) | 46.91 MiB/s (45.06–47.80 MiB/s) | 43.08 MiB/s (35.78–43.10 MiB/s) | 80.70 MiB/s (76.90–83.43 MiB/s) |

</details>

并发测试使用 rust-raknet 默认的 1,400 B 名义 MTU；quic-go UDP 预算为 1,372 B，对应同样的 1,400 B IPv4 包预算，并关闭路径 MTU 发现。C KCP 使用 1,400 B UDP MTU，实际包预算多 28 B，但两种测试消息都不需要分片。TCP 可能把多条记录合并到同一个报文段中，所以相同的包丢失率不代表各实现丢失相同数量的应用消息。

### 持续负载下的交付延迟

使用独立的采样驱动，在消息发送前记录时间戳，在整个突发发送过程中收到并校验回显后测量 RTT。应用窗口仍为 16 条，工作线程数、CPU 绑定和服务端模式不变，但每轮校验 262,144 条突发回显。延迟包含客户端和服务端的排队时间。采样代码会增加开销，因此其吞吐数据没有混入上面的吞吐表。结果取三轮测试的中位数，每个单元格展示**中位 RTT / 99% 分位 RTT**，越低越好。

| 消息大小 | 连接数 | 注入丢包率 | TCP | rust-raknet (`send`) | rust-raknet（批量接口） |
| --- | --- | --- | --- | --- | --- |
| 64 B | 64 条连接 | 0% | 0.94 ms / 5.28 ms | 1.00 ms / 7.84 ms | 0.45 ms / 3.71 ms |
| 64 B | 1,024 条连接 | 0% | 19.12 ms / 38.37 ms | 15.99 ms / 42.99 ms | 4.21 ms / 22.42 ms |
| 64 B | 64 条连接 | 1% | 0.65 ms / 3.74 ms | 0.94 ms / 5.83 ms | 0.95 ms / 4.85 ms |
| 64 B | 1,024 条连接 | 1% | 15.71 ms / 37.28 ms | 15.44 ms / 62.46 ms | 10.22 ms / 51.70 ms |
| 800 B | 64 条连接 | 0% | 1.15 ms / 9.62 ms | 1.10 ms / 8.51 ms | 1.11 ms / 9.22 ms |
| 800 B | 1,024 条连接 | 0% | 20.37 ms / 40.35 ms | 19.24 ms / 124.51 ms | 17.59 ms / 113.04 ms |
| 800 B | 64 条连接 | 1% | 0.66 ms / 3.95 ms | 0.99 ms / 5.90 ms | 1.05 ms / 5.92 ms |
| 800 B | 1,024 条连接 | 1% | 18.34 ms / 48.50 ms | 18.23 ms / 71.80 ms | 20.38 ms / 100.26 ms |

### 如何理解这些结果

- 64 B 消息、无注入丢包时，批量接口在 64 连接下把并发吞吐从 47.59 提高到 129.31 MiB/s，在 1,024 连接下从 49.03 提高到 148.73 MiB/s。独立的 1,024 连接负载延迟测试中，中位 RTT 从 15.99 降到 4.21 ms，99% 分位 RTT 从 42.99 降到 22.42 ms。
- 单连接 64 B 批量结果不够稳定：中位数为 19.81 MiB/s，范围为 16.45–35.17 MiB/s，没有稳定重现上一轮的小包收益。独立诊断构建在十轮无注入丢包测试中，有四轮收到匹配的 NACK 并停止合包，尽管 netem 丢包和 UDP 接收缓冲区丢包均为零。NACK 表示序号出现缺口，本身不能证明数据包永久丢失。当前永久回退策略使批量收益对这种反馈较敏感；诊断结果未混入表格。
- 普通 rust-raknet 接口在单连接 800 B 场景下仍有较高吞吐，包括 1% 和 5% 丢包组。TCP 领先单连接 4 KiB 组和无注入丢包的 800 B 并发组。quic-go 领先 64 B / 64 连接组和两个 64 B 并发丢包组，没有一种实现赢得所有场景。
- 批量接口**不是通用提速开关**。800 B、2,048 连接、无注入丢包时，批量接口为 317.01 MiB/s，普通接口为 437.78 MiB/s，低约 28%；64 B、单连接、5% 丢包时分别为 11.98 和 14.81 MiB/s，低约 19%。即使无法合包或合包已关闭，接口、批量接收和驱动缓冲区处理的差异仍会影响结果。
- 两种模式的稀疏请求中位 RTT 接近：`send` 为 17.6 µs，批量接口为 17.4 µs。但负载延迟并非始终更低：800 B、1,024 连接、1% 丢包时，批量模式的 99% 分位 RTT 为 100.26 ms，普通模式为 71.80 ms。应根据实际消息大小、并发量和延迟分布选择接口。


每轮均记录随机 netem 丢包及网络命名空间内的 UDP 接收缓冲区错误。UDP socket 饱和时，即使没有注入丢包也可能丢包。单轮吞吐测试中最大的接收缓冲区丢包数为：rust-raknet（`send`）187,997 个数据报、rust-raknet（批量接口）375,102 个数据报、C KCP 335,992 个数据报、quic-go 6,124 个数据报。这些丢包属于实测负载的一部分。随机丢包和共享 CPU 调度会影响结果，应结合范围判断，不能把较小的中位数差异当作通用排名。

全部 397 次测量均完成回包校验：120 次单连接吞吐、25 次稀疏延迟、180 次并发吞吐和 72 次负载延迟。这批结果替换了此前的表格，并不是与旧版库进行严格控制条件的前后对比。

构建命令、接口参数和负载细节见 [benchmark README](examples/test_benchmark/README.md) 和 [C KCP 测试适配器](examples/test_benchmark/kcp/README.md)。

## 参与贡献

提交问题时，请附上 crate 版本、尽可能小的复现示例，以及预期行为。较大的改动建议先开 issue 讨论，再开始写代码。

开发应在一次性容器、虚拟机或任务专属副本中完成，并使用独立缓存。网络测试应在独立网络命名空间中运行。提交 pull request 前，请执行：

```sh
cargo fmt --all -- --check
cargo build --all-targets
cargo test --all-targets
```

请说明执行了哪些检查。涉及传输或性能的改动，还应提供测试负载和足够的复现信息。

更新版本时，在任务副本中运行 `python3 scripts/set-version.py NEW_VERSION`，使用下一次发布的版本号，例如 `0.16.0`。该脚本会同时更新 `Cargo.toml`、中英文 README 的依赖示例和 crate 文档，CI 会检查它们是否一致。

### 贡献者

感谢所有提交过代码的贡献者：

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

本项目采用 [MIT](LICENSE) 协议。协议细节见 [RakNet 协议参考](http://www.jenkinssoftware.com/raknet/manual/index.html)。本项目与 Jenkins Software LLC 或 RakNet 没有关联。
