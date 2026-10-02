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
rust-raknet = "1.0.0"
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

截至 2026 年 10 月 2 日，基岩版最新稳定版本为 [26.52](https://feedback.minecraft.net/hc/en-us/articles/49175370527501-Minecraft-Bedrock-Edition-26-52-Hotfix-Changelog)。已测试的环境使用 Windows 1.26.52 客户端和基岩版专用服务端 1.26.52.3，游戏协议版本为 2193，RakNet 协议版本为 11。通过 `rust-raknet` 1.0.0 的 RakNet UDP 代理，加入世界、走动、放置和破坏方块，以及服务器列表 MOTD 均正常。

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
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 0.16.0，提交 `d65aee3`；另增测试采样代码 |
| rust-raknet（批量接口） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 与 `send` 列使用同一份库构建 |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | 固定提交 `b1a7a21`，未修改上游 C 核心 |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0; Go 1.27.1 |

- **`send`**：两端均使用普通 `send` / `recv`，不调用批量接口。
- **批量接口**：单连接客户端使用 `send_batch`，并发客户端使用 `send_bytes_batch`；回显服务端使用 `recv_bytes_batch` 和 `send_bytes_batch`。每次调用最多提交 16 条已准备好的消息，两端都不等待凑满批次。

每次测试都会启动新的服务端并建立新连接。两列 rust-raknet 均使用 `ReliableOrdered`、正常的 64 帧可靠在途上限和正常重试设置。可选的 UDP 批量接收和空闲维护均关闭。消息合并与 UDP 系统调用的批量发送是不同机制，后者在两种接口模式中都可以使用。

在 MTU 范围内，一个 UDP 数据报最多合并**八条**不需要分片的消息。测试使用的 MTU 放不下两条 800 B 消息；4,096 B 消息需要分片，也不会合包。这些批量列衡量的是批量接口和批量接收路径，而不是消息合并。收到匹配的 NACK 或发生可靠消息重传超时后，该连接会永久停止合包，包括预热阶段发生的反馈。因此，**调用批量接口不等于每轮测试都实际合并了消息**。

TCP 使用 `TCP_NODELAY`、四字节小端长度前缀、整条记录写入，并复用缓冲区。C KCP 使用消息模式，发送/接收分段窗口在单连接测试中为 64/64，并发测试中为 64/128，配置 `nodelay(1, 10, 2, 1)`，立即写入和确认，不使用 FEC 或加密。QUIC 每条连接使用一个双向可靠有序流，记录格式与 TCP 相同，保留加密、拥塞控制和流量控制；rust-raknet 和 C KCP 不提供同样的功能。

吞吐量按校验通过的回显应用数据计算，统计**单方向**，不包含包头和 ACK，**越高越好**。表格展示三轮测试的中位数，五列实现的运行顺序轮换。1 MiB = 1,048,576 B。每轮使用独立随机丢包，数据和 ACK 的两个方向均受影响。

表中数值为各轮测量的中位数，括号内为最小–最大范围，所有数值均带单位。吞吐表每格依次显示吞吐量、客户端（**C**）和服务端（**S**）峰值 RSS；延迟表把同一轮负载的范围及内存一起列出。

RSS 使用 Linux `wait4` 记录进程峰值，覆盖启动、建连、预热、负载和退出，包含运行时、分配器及应用缓冲区，不包含内核 socket 缓冲区。TCP 协议状态位于内核中，RSS 不代表全部网络内存。C / S 分别统计，不是每连接内存；两端峰值不一定同时发生。

### 单连接吞吐量

所有客户端最多允许 64 条应用消息等待回显。每轮在吞吐测试前执行 100 次预热和 300 次串行 RTT 采样。Rust 和 Go 使用四个工作线程，C KCP 每个进程使用一个事件循环。吞吐测试不绑定 CPU，因此这些结果不是相同 CPU 成本下的效率比较。

| 网络条件 | 消息大小 / 突发数量 | TCP<br>吞吐量（范围）<br>RSS：C / S | rust-raknet (`send`)<br>吞吐量（范围）<br>RSS：C / S | rust-raknet （批量接口）<br>吞吐量（范围）<br>RSS：C / S | C KCP<br>吞吐量（范围）<br>RSS：C / S | quic-go<br>吞吐量（范围）<br>RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- |
| 无注入丢包 | 800 B / 200,000 条消息 | 150.71 MiB/s (149.60–151.39 MiB/s)<br>C: 3.53 MiB (3.46–3.54 MiB)<br>S: 3.45 MiB (3.37–3.45 MiB) | 188.34 MiB/s (183.93–191.66 MiB/s)<br>C: 3.84 MiB (3.69–3.98 MiB)<br>S: 3.77 MiB (3.60–3.89 MiB) | 193.30 MiB/s (189.46–195.30 MiB/s)<br>C: 3.85 MiB (3.71–3.99 MiB)<br>S: 3.88 MiB (3.67–4.00 MiB) | 152.98 MiB/s (151.66–155.92 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 116.06 MiB/s (115.70–117.91 MiB/s)<br>C: 14.15 MiB (14.14–14.41 MiB)<br>S: 13.70 MiB (13.55–14.21 MiB) |
| 1% 丢包 | 800 B / 50,000 条消息 | 17.02 MiB/s (14.55–19.00 MiB/s)<br>C: 3.65 MiB (3.42–3.74 MiB)<br>S: 3.44 MiB (3.29–3.50 MiB) | 151.54 MiB/s (151.06–151.61 MiB/s)<br>C: 3.70 MiB (3.70–3.82 MiB)<br>S: 3.82 MiB (3.54–3.85 MiB) | 158.37 MiB/s (151.12–162.72 MiB/s)<br>C: 3.78 MiB (3.59–3.99 MiB)<br>S: 3.95 MiB (3.79–4.02 MiB) | 131.88 MiB/s (129.13–132.70 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 77.07 MiB/s (29.79–87.86 MiB/s)<br>C: 13.75 MiB (13.49–13.85 MiB)<br>S: 13.52 MiB (13.50–13.56 MiB) |
| 5% 丢包 | 800 B / 50,000 条消息 | 1.06 MiB/s (0.99–1.11 MiB/s)<br>C: 3.55 MiB (3.41–3.77 MiB)<br>S: 3.41 MiB (3.29–3.45 MiB) | 146.94 MiB/s (140.16–149.13 MiB/s)<br>C: 3.85 MiB (3.81–3.91 MiB)<br>S: 3.78 MiB (3.39–3.98 MiB) | 142.99 MiB/s (133.51–144.10 MiB/s)<br>C: 3.83 MiB (3.72–4.10 MiB)<br>S: 3.84 MiB (3.74–3.86 MiB) | 113.61 MiB/s (111.54–124.69 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) | 10.03 MiB/s (8.66–13.69 MiB/s)<br>C: 13.68 MiB (13.64–13.78 MiB)<br>S: 14.00 MiB (13.89–14.31 MiB) |
| 无注入丢包 | 64 B / 300,000 条消息 | 13.82 MiB/s (13.32–14.09 MiB/s)<br>C: 3.58 MiB (3.41–3.75 MiB)<br>S: 3.40 MiB (3.29–3.49 MiB) | 16.45 MiB/s (16.25–16.66 MiB/s)<br>C: 3.58 MiB (3.50–3.79 MiB)<br>S: 3.70 MiB (3.69–3.78 MiB) | 49.83 MiB/s (49.04–50.50 MiB/s)<br>C: 3.58 MiB (3.57–3.73 MiB)<br>S: 3.71 MiB (3.52–3.73 MiB) | 12.56 MiB/s (12.43–12.72 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 31.30 MiB/s (10.36–31.48 MiB/s)<br>C: 13.51 MiB (13.25–13.89 MiB)<br>S: 13.51 MiB (13.32–14.20 MiB) |
| 1% 丢包 | 64 B / 100,000 条消息 | 2.34 MiB/s (1.63–3.86 MiB/s)<br>C: 3.58 MiB (3.36–3.63 MiB)<br>S: 3.34 MiB (3.25–3.51 MiB) | 15.13 MiB/s (14.65–16.43 MiB/s)<br>C: 3.86 MiB (3.73–3.89 MiB)<br>S: 3.80 MiB (3.70–3.83 MiB) | 14.91 MiB/s (14.85–16.35 MiB/s)<br>C: 3.73 MiB (3.63–3.89 MiB)<br>S: 3.71 MiB (3.68–3.74 MiB) | 11.45 MiB/s (11.34–11.72 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 13.69 MiB/s (8.00–21.54 MiB/s)<br>C: 12.64 MiB (12.01–12.76 MiB)<br>S: 11.95 MiB (11.57–12.07 MiB) |
| 5% 丢包 | 64 B / 100,000 条消息 | 0.11 MiB/s (0.11–0.13 MiB/s)<br>C: 3.58 MiB (3.46–3.63 MiB)<br>S: 3.35 MiB (3.33–3.50 MiB) | 13.42 MiB/s (13.26–13.63 MiB/s)<br>C: 3.62 MiB (3.59–3.62 MiB)<br>S: 3.66 MiB (3.48–3.77 MiB) | 13.92 MiB/s (13.91–14.32 MiB/s)<br>C: 3.79 MiB (3.78–3.82 MiB)<br>S: 3.81 MiB (3.60–3.89 MiB) | 10.16 MiB/s (9.96–10.19 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 3.15 MiB/s (2.73–3.20 MiB/s)<br>C: 12.64 MiB (12.51–12.76 MiB)<br>S: 12.13 MiB (12.07–12.20 MiB) |
| 无注入丢包 | 4,096 B / 50,000 条消息 | 345.98 MiB/s (342.74–347.49 MiB/s)<br>C: 3.59 MiB (3.52–3.61 MiB)<br>S: 3.45 MiB (3.43–3.49 MiB) | 314.30 MiB/s (301.29–315.82 MiB/s)<br>C: 4.71 MiB (4.43–4.76 MiB)<br>S: 4.68 MiB (4.64–4.76 MiB) | 313.17 MiB/s (310.92–315.55 MiB/s)<br>C: 4.66 MiB (4.65–4.89 MiB)<br>S: 4.80 MiB (4.67–5.02 MiB) | 256.66 MiB/s (249.47–263.40 MiB/s)<br>C: 1.87 MiB (1.86–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 227.94 MiB/s (149.56–228.93 MiB/s)<br>C: 14.23 MiB (14.18–14.24 MiB)<br>S: 13.73 MiB (13.72–14.10 MiB) |
| 1% 丢包 + 单向 5 ms 延迟 | 800 B / 3,000 条消息 | 1.21 MiB/s (1.09–1.28 MiB/s)<br>C: 3.58 MiB (3.56–3.59 MiB)<br>S: 3.34 MiB (3.26–3.45 MiB) | 2.91 MiB/s (2.72–2.96 MiB/s)<br>C: 3.80 MiB (3.78–3.98 MiB)<br>S: 3.68 MiB (3.64–3.70 MiB) | 3.05 MiB/s (2.99–3.12 MiB/s)<br>C: 3.79 MiB (3.61–3.89 MiB)<br>S: 3.71 MiB (3.65–3.80 MiB) | 2.85 MiB/s (2.79–2.95 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 1.67 MiB/s (1.55–1.79 MiB/s)<br>C: 11.83 MiB (11.64–11.89 MiB)<br>S: 11.45 MiB (10.70–11.57 MiB) |

单连接测试中，各 UDP 实现的包大小预算一致：rust-raknet 名义 MTU 为 1,428 B，包含 28 B IPv4/UDP 开销；C KCP 和 quic-go 的 UDP 数据报预算为 1,400 B。QUIC 的路径 MTU 发现已关闭。每条 4,096 B 消息在 rust-raknet 和 C KCP 中都分成三片；本库默认名义 MTU 仍为 1,400 B。

### 稀疏请求的往返延迟

**越低越好。** 下表取五轮测试中各轮 RTT 分位值的中位数；每轮包含 1,000 次预热和 10,000 次串行采样，消息大小为 800 B，不注入丢包。客户端和服务端分别绑定不同 CPU；每个 Rust/Go 进程在指定 CPU 上运行四个工作线程，C KCP 使用一个事件循环。CPU 绑定不代表独占 CPU。

| 实现 / 接口 | 中位 RTT（范围） | 95% 分位 RTT（范围） | 99% 分位 RTT（范围） | 峰值 RSS：C / S（范围） |
| --- | --- | --- | --- | --- |
| TCP | 13.0 µs (12.8–13.1 µs) | 19.8 µs (19.5–21.6 µs) | 37.3 µs (37.1–39.5 µs) | C: 3.87 MiB (3.84–3.98 MiB)<br>S: 3.47 MiB (3.45–3.48 MiB) |
| rust-raknet (`send`) | 18.5 µs (17.2–18.6 µs) | 24.4 µs (22.6–32.9 µs) | 48.9 µs (45.0–53.3 µs) | C: 4.31 MiB (4.20–4.36 MiB)<br>S: 4.12 MiB (3.90–4.25 MiB) |
| rust-raknet （批量接口） | 18.2 µs (17.4–18.4 µs) | 22.4 µs (21.7–25.2 µs) | 47.7 µs (45.9–49.3 µs) | C: 4.16 MiB (4.15–4.39 MiB)<br>S: 4.01 MiB (3.90–4.08 MiB) |
| C KCP | 10.4 µs (10.4–10.5 µs) | 16.0 µs (14.2–16.3 µs) | 32.6 µs (27.1–35.2 µs) | C: 1.88 MiB (1.87–1.88 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) |
| quic-go | 62.0 µs (61.9–62.1 µs) | 96.7 µs (95.7–98.0 µs) | 132.6 µs (126.2–170.2 µs) | C: 13.26 MiB (12.95–13.57 MiB)<br>S: 11.01 MiB (10.95–11.07 MiB) |

这组稀疏采样中，批量客户端每次只向 `send_batch` 提交**一条**消息，批量服务端也不会等待更多消息后才回复，因此没有实际合包。结果衡量的是稀疏请求的接口路径，而非突发流量的交付延迟。并发吞吐测试也会输出突发发送前的 RTT，但下方负载延迟表不使用这些数据。

### 高并发吞吐量

每轮校验 **1,048,576 条有序回显**，每条连接的应用窗口为 16 条消息。每条回显都会校验连接 ID、消息 ID 和有效载荷。所有连接先各自完成 20 次串行采样，再通过统一屏障开始吞吐测试；建连和采样时间不计入吞吐计时，所有连接保持打开，直到全部突发发送完成。

所有实现均使用四个工作线程，以及四个启用 `SO_REUSEPORT` 的服务端监听器或接收 socket。服务端绑定四个 CPU，客户端绑定另外四个，使用可靠有序的回显负载。

| 消息大小 | 连接数 | 注入丢包率 | TCP<br>吞吐量（范围）<br>RSS：C / S | rust-raknet (`send`)<br>吞吐量（范围）<br>RSS：C / S | rust-raknet （批量接口）<br>吞吐量（范围）<br>RSS：C / S | C KCP<br>吞吐量（范围）<br>RSS：C / S | quic-go<br>吞吐量（范围）<br>RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 条连接 | 0% | 686.84 MiB/s (660.28–696.38 MiB/s)<br>C: 3.96 MiB (3.94–3.96 MiB)<br>S: 3.48 MiB (3.29–3.50 MiB) | 566.35 MiB/s (513.44–608.05 MiB/s)<br>C: 7.19 MiB (6.83–7.57 MiB)<br>S: 8.07 MiB (7.33–8.23 MiB) | 570.96 MiB/s (536.42–613.69 MiB/s)<br>C: 7.25 MiB (7.23–7.62 MiB)<br>S: 7.61 MiB (7.44–7.97 MiB) | 385.96 MiB/s (381.21–415.65 MiB/s)<br>C: 3.00 MiB (3.00–3.00 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 230.52 MiB/s (223.93–232.42 MiB/s)<br>C: 29.70 MiB (29.16–29.77 MiB)<br>S: 33.91 MiB (31.39–36.24 MiB) |
| 800 B | 256 条连接 | 0% | 607.51 MiB/s (573.21–608.02 MiB/s)<br>C: 5.78 MiB (5.64–5.78 MiB)<br>S: 3.66 MiB (3.53–3.72 MiB) | 549.78 MiB/s (546.04–569.34 MiB/s)<br>C: 15.78 MiB (15.75–15.90 MiB)<br>S: 14.38 MiB (13.83–14.53 MiB) | 549.17 MiB/s (526.89–561.53 MiB/s)<br>C: 16.47 MiB (16.31–16.75 MiB)<br>S: 14.93 MiB (14.28–15.23 MiB) | 407.50 MiB/s (388.71–408.66 MiB/s)<br>C: 6.25 MiB (6.25–6.25 MiB)<br>S: 7.00 MiB (6.88–7.00 MiB) | 191.40 MiB/s (188.11–193.07 MiB/s)<br>C: 77.63 MiB (76.96–77.87 MiB)<br>S: 79.33 MiB (72.77–81.14 MiB) |
| 800 B | 1,024 条连接 | 0% | 579.63 MiB/s (552.38–580.28 MiB/s)<br>C: 12.33 MiB (11.49–12.47 MiB)<br>S: 4.88 MiB (4.73–4.95 MiB) | 543.38 MiB/s (510.86–547.88 MiB/s)<br>C: 47.03 MiB (46.72–47.24 MiB)<br>S: 39.34 MiB (37.62–41.84 MiB) | 424.12 MiB/s (397.64–514.45 MiB/s)<br>C: 48.34 MiB (48.27–49.49 MiB)<br>S: 40.75 MiB (40.61–43.16 MiB) | 384.08 MiB/s (381.90–400.60 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 20.63 MiB (20.25–20.63 MiB) | 173.05 MiB/s (163.57–173.73 MiB/s)<br>C: 242.27 MiB (241.52–244.64 MiB)<br>S: 219.83 MiB (207.71–223.86 MiB) |
| 800 B | 2,048 条连接 | 0% | 551.95 MiB/s (515.72–587.01 MiB/s)<br>C: 18.20 MiB (16.71–20.39 MiB)<br>S: 6.61 MiB (6.57–6.68 MiB) | 433.48 MiB/s (430.64–434.75 MiB/s)<br>C: 87.66 MiB (87.63–89.96 MiB)<br>S: 69.42 MiB (69.22–70.91 MiB) | 408.05 MiB/s (367.75–414.65 MiB/s)<br>C: 89.64 MiB (89.14–91.14 MiB)<br>S: 74.59 MiB (73.68–74.95 MiB) | 342.55 MiB/s (334.12–344.45 MiB/s)<br>C: 38.75 MiB (38.63–38.88 MiB)<br>S: 38.38 MiB (37.63–38.38 MiB) | 136.72 MiB/s (135.05–138.96 MiB/s)<br>C: 465.14 MiB (459.12–467.06 MiB)<br>S: 380.00 MiB (367.71–399.96 MiB) |
| 800 B | 64 条连接 | 1% | 426.91 MiB/s (340.17–500.90 MiB/s)<br>C: 4.10 MiB (3.91–4.14 MiB)<br>S: 3.33 MiB (3.09–3.50 MiB) | 537.82 MiB/s (523.14–538.61 MiB/s)<br>C: 7.25 MiB (7.10–7.47 MiB)<br>S: 7.72 MiB (7.68–7.87 MiB) | 539.56 MiB/s (521.51–557.75 MiB/s)<br>C: 7.45 MiB (7.37–7.63 MiB)<br>S: 7.98 MiB (7.78–8.13 MiB) | 388.20 MiB/s (380.95–402.75 MiB/s)<br>C: 2.88 MiB (2.75–3.00 MiB)<br>S: 3.13 MiB (3.13–3.25 MiB) | 219.77 MiB/s (216.52–220.89 MiB/s)<br>C: 28.51 MiB (28.32–28.76 MiB)<br>S: 33.42 MiB (31.50–33.46 MiB) |
| 800 B | 256 条连接 | 1% | 478.63 MiB/s (460.11–516.55 MiB/s)<br>C: 5.80 MiB (5.73–5.87 MiB)<br>S: 3.64 MiB (3.53–3.70 MiB) | 474.69 MiB/s (473.86–497.07 MiB/s)<br>C: 16.32 MiB (16.14–17.25 MiB)<br>S: 15.69 MiB (15.27–15.75 MiB) | 503.83 MiB/s (499.65–505.25 MiB/s)<br>C: 17.13 MiB (16.84–17.18 MiB)<br>S: 15.37 MiB (15.36–15.56 MiB) | 390.50 MiB/s (327.94–418.43 MiB/s)<br>C: 6.25 MiB (6.25–6.38 MiB)<br>S: 6.88 MiB (6.75–6.88 MiB) | 183.33 MiB/s (182.52–185.20 MiB/s)<br>C: 75.93 MiB (75.65–76.08 MiB)<br>S: 72.21 MiB (71.33–73.33 MiB) |
| 800 B | 1,024 条连接 | 1% | 495.35 MiB/s (495.10–512.14 MiB/s)<br>C: 12.21 MiB (12.04–12.55 MiB)<br>S: 4.72 MiB (4.66–4.72 MiB) | 445.63 MiB/s (436.74–471.55 MiB/s)<br>C: 47.18 MiB (46.41–49.34 MiB)<br>S: 42.86 MiB (42.12–43.16 MiB) | 433.00 MiB/s (412.66–444.47 MiB/s)<br>C: 49.04 MiB (48.64–51.79 MiB)<br>S: 45.11 MiB (42.27–45.48 MiB) | 324.89 MiB/s (313.60–384.79 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 19.88 MiB (18.88–20.63 MiB) | 79.09 MiB/s (77.68–97.35 MiB/s)<br>C: 329.10 MiB (323.22–339.30 MiB)<br>S: 194.25 MiB (190.52–199.92 MiB) |
| 800 B | 2,048 条连接 | 1% | 425.19 MiB/s (255.63–440.72 MiB/s)<br>C: 17.40 MiB (17.23–20.09 MiB)<br>S: 6.60 MiB (6.43–6.64 MiB) | 371.13 MiB/s (341.29–387.26 MiB/s)<br>C: 90.00 MiB (88.92–92.36 MiB)<br>S: 74.96 MiB (73.77–77.34 MiB) | 324.86 MiB/s (323.00–328.90 MiB/s)<br>C: 92.60 MiB (92.43–92.98 MiB)<br>S: 77.47 MiB (75.64–77.80 MiB) | 300.43 MiB/s (270.36–316.49 MiB/s)<br>C: 38.75 MiB (38.75–39.00 MiB)<br>S: 37.38 MiB (36.13–38.00 MiB) | 36.02 MiB/s (34.86–38.96 MiB/s)<br>C: 760.16 MiB (753.87–775.20 MiB)<br>S: 411.05 MiB (403.68–418.38 MiB) |
| 64 B | 64 条连接 | 0% | 66.29 MiB/s (63.73–74.45 MiB/s)<br>C: 3.98 MiB (3.70–4.09 MiB)<br>S: 3.27 MiB (3.25–3.57 MiB) | 50.20 MiB/s (49.10–51.96 MiB/s)<br>C: 5.64 MiB (5.58–5.86 MiB)<br>S: 5.85 MiB (5.75–5.89 MiB) | 112.17 MiB/s (97.73–112.77 MiB/s)<br>C: 5.71 MiB (5.69–5.91 MiB)<br>S: 6.05 MiB (6.01–6.43 MiB) | 34.92 MiB/s (30.01–36.24 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.38–2.50 MiB) | 124.82 MiB/s (117.80–139.59 MiB/s)<br>C: 21.71 MiB (21.24–22.41 MiB)<br>S: 19.02 MiB (18.83–19.58 MiB) |
| 64 B | 1,024 条连接 | 0% | 55.28 MiB/s (54.29–56.05 MiB/s)<br>C: 9.24 MiB (9.09–9.31 MiB)<br>S: 4.14 MiB (4.12–4.36 MiB) | 48.66 MiB/s (47.83–50.19 MiB/s)<br>C: 32.75 MiB (32.27–32.95 MiB)<br>S: 27.39 MiB (25.48–28.52 MiB) | 142.12 MiB/s (140.84–146.14 MiB/s)<br>C: 32.24 MiB (31.98–32.27 MiB)<br>S: 25.79 MiB (25.66–25.85 MiB) | 42.03 MiB/s (41.17–44.17 MiB/s)<br>C: 8.88 MiB (8.88–8.88 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 109.34 MiB/s (108.98–109.42 MiB/s)<br>C: 147.46 MiB (131.64–148.71 MiB)<br>S: 96.83 MiB (94.08–96.85 MiB) |
| 64 B | 64 条连接 | 1% | 42.97 MiB/s (41.55–44.56 MiB/s)<br>C: 4.01 MiB (3.80–4.02 MiB)<br>S: 3.34 MiB (3.22–3.36 MiB) | 47.57 MiB/s (46.08–48.15 MiB/s)<br>C: 5.80 MiB (5.73–5.87 MiB)<br>S: 6.18 MiB (5.95–6.24 MiB) | 46.27 MiB/s (41.98–48.32 MiB/s)<br>C: 6.04 MiB (5.93–6.07 MiB)<br>S: 6.33 MiB (6.29–6.52 MiB) | 32.18 MiB/s (30.46–34.99 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.50 MiB (2.38–2.50 MiB) | 62.27 MiB/s (61.10–65.93 MiB/s)<br>C: 21.81 MiB (21.58–21.88 MiB)<br>S: 18.96 MiB (18.80–19.27 MiB) |
| 64 B | 1,024 条连接 | 1% | 51.20 MiB/s (46.05–53.09 MiB/s)<br>C: 9.13 MiB (9.09–9.58 MiB)<br>S: 4.09 MiB (3.91–4.11 MiB) | 43.49 MiB/s (42.68–43.57 MiB/s)<br>C: 34.34 MiB (33.05–35.57 MiB)<br>S: 29.52 MiB (28.13–29.59 MiB) | 49.85 MiB/s (49.79–52.47 MiB/s)<br>C: 35.72 MiB (35.22–35.84 MiB)<br>S: 30.05 MiB (29.84–30.09 MiB) | 40.47 MiB/s (37.55–42.08 MiB/s)<br>C: 8.75 MiB (8.75–8.75 MiB)<br>S: 9.25 MiB (9.13–9.25 MiB) | 81.37 MiB/s (81.09–90.10 MiB/s)<br>C: 132.64 MiB (130.52–142.89 MiB)<br>S: 95.26 MiB (93.71–96.21 MiB) |

并发测试使用 rust-raknet 默认的 1,400 B 名义 MTU；quic-go UDP 预算为 1,372 B，对应同样的 1,400 B IPv4 包预算，并关闭路径 MTU 发现。C KCP 使用 1,400 B UDP MTU，实际包预算多 28 B，但两种测试消息都不需要分片。TCP 可能把多条记录合并到同一个报文段中，所以相同的包丢失率不代表各实现丢失相同数量的应用消息。

### 持续负载下的交付延迟

所有并发客户端均支持 `--loaded-rtt`：每条消息发送前记录时间戳，整个突发过程中收到并校验回显后记录 RTT。应用窗口仍为 16 条，工作线程数、CPU 绑定和服务端模式不变，每轮校验 262,144 条突发回显。延迟包含客户端和服务端的排队时间。采样代码会增加开销，因此其吞吐数据没有混入上面的吞吐表。结果取三轮测试的中位数，分别展示**中位 RTT、99% 分位 RTT**及其范围，越低越好。表中 RSS 包含客户端保存、汇总和排序 RTT 样本的内存开销。

| 消息大小 | 连接数 | 注入丢包率 | TCP<br>RTT 中位 / 99% 分位（范围）<br>RSS：C / S | rust-raknet (`send`)<br>RTT 中位 / 99% 分位（范围）<br>RSS：C / S | rust-raknet （批量接口）<br>RTT 中位 / 99% 分位（范围）<br>RSS：C / S | C KCP<br>RTT 中位 / 99% 分位（范围）<br>RSS：C / S | quic-go<br>RTT 中位 / 99% 分位（范围）<br>RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 条连接 | 0% | 50%: 0.91 ms (0.88–0.95 ms)<br>99%: 3.66 ms (3.51–4.39 ms)<br>C: 11.54 MiB (11.46–11.61 MiB)<br>S: 3.28 MiB (3.22–3.32 MiB) | 50%: 1.04 ms (1.02–1.06 ms)<br>99%: 4.30 ms (3.39–8.22 ms)<br>C: 13.38 MiB (13.26–13.43 MiB)<br>S: 5.77 MiB (5.57–5.86 MiB) | 50%: 0.40 ms (0.37–0.43 ms)<br>99%: 3.99 ms (1.62–4.30 ms)<br>C: 13.38 MiB (13.28–13.46 MiB)<br>S: 5.90 MiB (5.74–5.99 MiB) | 50%: 1.32 ms (1.26–1.36 ms)<br>99%: 6.61 ms (5.66–9.60 ms)<br>C: 6.21 MiB (6.20–6.26 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.36 ms (0.35–0.36 ms)<br>99%: 2.49 ms (1.55–2.89 ms)<br>C: 30.30 MiB (27.44–31.44 MiB)<br>S: 17.33 MiB (17.21–17.71 MiB) |
| 64 B | 1,024 条连接 | 0% | 50%: 17.27 ms (17.04–17.89 ms)<br>99%: 36.27 ms (35.31–39.06 ms)<br>C: 16.73 MiB (16.35–17.03 MiB)<br>S: 4.13 MiB (4.00–4.23 MiB) | 50%: 15.99 ms (15.70–18.16 ms)<br>99%: 87.61 ms (36.52–94.68 ms)<br>C: 36.47 MiB (36.36–36.68 MiB)<br>S: 23.73 MiB (22.76–23.86 MiB) | 50%: 4.76 ms (4.53–5.00 ms)<br>99%: 19.49 ms (17.86–19.88 ms)<br>C: 37.91 MiB (37.82–38.02 MiB)<br>S: 24.41 MiB (24.10–24.41 MiB) | 50%: 5.12 ms (5.12–11.34 ms)<br>99%: 161.62 ms (101.00–162.49 ms)<br>C: 10.69 MiB (10.57–10.94 MiB)<br>S: 9.25 MiB (9.25–9.50 MiB) | 50%: 8.89 ms (8.67–8.94 ms)<br>99%: 14.09 ms (12.27–35.74 ms)<br>C: 138.33 MiB (138.08–139.46 MiB)<br>S: 86.21 MiB (84.71–92.96 MiB) |
| 64 B | 64 条连接 | 1% | 50%: 0.63 ms (0.59–0.68 ms)<br>99%: 3.41 ms (2.85–3.47 ms)<br>C: 11.62 MiB (11.61–11.82 MiB)<br>S: 3.43 MiB (3.39–3.46 MiB) | 50%: 0.94 ms (0.91–0.97 ms)<br>99%: 5.70 ms (5.18–5.81 ms)<br>C: 13.63 MiB (13.42–13.75 MiB)<br>S: 6.02 MiB (5.81–6.14 MiB) | 50%: 0.97 ms (0.91–0.99 ms)<br>99%: 5.12 ms (4.80–5.99 ms)<br>C: 13.71 MiB (13.69–13.92 MiB)<br>S: 6.18 MiB (6.11–6.41 MiB) | 50%: 1.23 ms (1.22–1.29 ms)<br>99%: 7.13 ms (5.39–8.05 ms)<br>C: 6.21 MiB (6.17–6.23 MiB)<br>S: 2.38 MiB (2.38–2.50 MiB) | 50%: 0.09 ms (0.09–0.10 ms)<br>99%: 27.22 ms (27.18–27.24 ms)<br>C: 33.52 MiB (30.96–34.39 MiB)<br>S: 17.46 MiB (16.96–17.57 MiB) |
| 64 B | 1,024 条连接 | 1% | 50%: 15.82 ms (14.79–16.82 ms)<br>99%: 35.74 ms (34.77–38.60 ms)<br>C: 17.36 MiB (16.71–17.39 MiB)<br>S: 4.25 MiB (4.14–4.25 MiB) | 50%: 16.12 ms (15.94–16.48 ms)<br>99%: 73.30 ms (59.21–90.99 ms)<br>C: 35.66 MiB (35.43–37.98 MiB)<br>S: 25.35 MiB (24.98–25.75 MiB) | 50%: 12.54 ms (11.93–13.24 ms)<br>99%: 34.82 ms (33.37–44.75 ms)<br>C: 38.88 MiB (37.43–39.15 MiB)<br>S: 25.90 MiB (25.77–25.91 MiB) | 50%: 6.84 ms (6.51–8.46 ms)<br>99%: 157.42 ms (122.50–160.02 ms)<br>C: 10.76 MiB (10.69–10.82 MiB)<br>S: 9.13 MiB (9.13–9.25 MiB) | 50%: 9.80 ms (9.11–10.45 ms)<br>99%: 55.46 ms (52.74–66.84 ms)<br>C: 133.46 MiB (133.14–137.96 MiB)<br>S: 87.45 MiB (87.18–89.26 MiB) |
| 800 B | 64 条连接 | 0% | 50%: 1.16 ms (1.12–1.17 ms)<br>99%: 4.95 ms (4.14–10.78 ms)<br>C: 11.84 MiB (11.81–12.00 MiB)<br>S: 3.22 MiB (3.21–3.38 MiB) | 50%: 1.11 ms (1.09–1.17 ms)<br>99%: 3.81 ms (3.35–3.89 ms)<br>C: 14.66 MiB (14.55–14.79 MiB)<br>S: 7.14 MiB (6.98–7.64 MiB) | 50%: 1.12 ms (1.09–1.17 ms)<br>99%: 5.94 ms (3.28–8.83 ms)<br>C: 14.79 MiB (14.64–14.91 MiB)<br>S: 7.30 MiB (7.28–7.46 MiB) | 50%: 1.18 ms (1.17–1.22 ms)<br>99%: 23.48 ms (6.77–30.71 ms)<br>C: 7.02 MiB (6.96–7.11 MiB)<br>S: 3.13 MiB (3.13–3.13 MiB) | 50%: 3.11 ms (3.08–3.14 ms)<br>99%: 14.75 ms (12.52–15.22 ms)<br>C: 36.69 MiB (36.36–38.85 MiB)<br>S: 32.20 MiB (30.51–32.23 MiB) |
| 800 B | 1,024 条连接 | 0% | 50%: 19.53 ms (19.52–21.90 ms)<br>99%: 40.40 ms (37.69–41.09 ms)<br>C: 20.02 MiB (20.00–20.07 MiB)<br>S: 4.92 MiB (4.91–5.05 MiB) | 50%: 13.14 ms (13.02–15.01 ms)<br>99%: 121.29 ms (116.86–193.91 ms)<br>C: 50.09 MiB (49.30–50.50 MiB)<br>S: 34.84 MiB (34.50–34.88 MiB) | 50%: 11.04 ms (10.65–18.39 ms)<br>99%: 184.17 ms (116.17–190.44 ms)<br>C: 51.06 MiB (50.85–52.21 MiB)<br>S: 34.95 MiB (34.54–36.98 MiB) | 50%: 17.84 ms (17.11–19.91 ms)<br>99%: 116.16 ms (113.56–120.76 ms)<br>C: 22.19 MiB (22.07–22.19 MiB)<br>S: 20.25 MiB (20.13–20.88 MiB) | 50%: 70.44 ms (69.77–72.96 ms)<br>99%: 124.10 ms (113.07–140.60 ms)<br>C: 227.46 MiB (226.21–237.71 MiB)<br>S: 156.83 MiB (151.87–162.71 MiB) |
| 800 B | 64 条连接 | 1% | 50%: 0.73 ms (0.68–0.79 ms)<br>99%: 5.66 ms (4.05–8.74 ms)<br>C: 11.75 MiB (11.65–12.10 MiB)<br>S: 3.38 MiB (3.38–3.39 MiB) | 50%: 1.07 ms (1.06–1.10 ms)<br>99%: 5.32 ms (5.02–7.29 ms)<br>C: 14.86 MiB (14.77–15.00 MiB)<br>S: 7.37 MiB (7.28–7.39 MiB) | 50%: 1.06 ms (1.04–1.10 ms)<br>99%: 5.98 ms (5.66–6.38 ms)<br>C: 15.10 MiB (15.08–15.12 MiB)<br>S: 7.72 MiB (7.51–7.82 MiB) | 50%: 1.16 ms (1.12–1.27 ms)<br>99%: 8.17 ms (7.62–15.36 ms)<br>C: 6.89 MiB (6.77–6.95 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.06 ms (3.02–3.06 ms)<br>99%: 13.40 ms (12.61–13.63 ms)<br>C: 37.86 MiB (36.83–38.06 MiB)<br>S: 29.83 MiB (29.78–34.01 MiB) |
| 800 B | 1,024 条连接 | 1% | 50%: 18.53 ms (18.27–18.59 ms)<br>99%: 46.00 ms (45.15–51.12 ms)<br>C: 20.23 MiB (20.10–20.32 MiB)<br>S: 4.86 MiB (4.73–4.91 MiB) | 50%: 15.22 ms (14.46–17.90 ms)<br>99%: 120.08 ms (68.63–121.71 ms)<br>C: 49.66 MiB (48.44–50.02 MiB)<br>S: 35.99 MiB (35.46–37.51 MiB) | 50%: 16.70 ms (15.52–18.56 ms)<br>99%: 116.65 ms (98.59–147.61 ms)<br>C: 51.54 MiB (50.38–52.66 MiB)<br>S: 36.85 MiB (36.84–37.36 MiB) | 50%: 18.00 ms (16.27–19.79 ms)<br>99%: 124.25 ms (120.94–127.48 ms)<br>C: 22.19 MiB (22.19–22.32 MiB)<br>S: 19.88 MiB (19.38–20.38 MiB) | 50%: 67.98 ms (67.98–69.97 ms)<br>99%: 174.05 ms (172.49–319.53 ms)<br>C: 243.64 MiB (243.08–245.33 MiB)<br>S: 198.14 MiB (181.47–228.02 MiB) |

### 如何理解这些结果

- 64 B 消息、1,024 连接、无注入丢包时，普通 rust-raknet 为 48.66 MiB/s，批量接口为 142.12 MiB/s；TCP 为 55.28 MiB/s，C KCP 为 42.03 MiB/s，quic-go 为 109.34 MiB/s。
- 同一条件下，持续负载中位 RTT / 99% 分位 RTT：普通接口为 15.99 ms / 87.61 ms，批量接口为 4.76 ms / 19.49 ms。五列负载延迟都采用同一逐消息采样方式。
- 吞吐和尾部延迟应分开看。800 B、1,024 连接、1% 丢包时，普通接口的 99% 分位 RTT 为 120.08 ms，批量接口为 116.65 ms。
- 800 B、2,048 连接、无注入丢包时，服务端峰值 RSS：TCP 为 6.61 MiB，普通 rust-raknet 为 69.42 MiB，批量接口为 74.59 MiB，C KCP 为 38.38 MiB，quic-go 为 380.00 MiB。这包含测试驱动和运行时，不是协议核心的堆内存对比。

并发批量客户端会为每批消息构造拥有所有权的缓冲区，普通客户端复用借用的消息模板。对比包含这些缓冲区选择和批量接收的影响，不能单独衡量合包成本。批量接口的收益与负载有关，尤其是在合包已回退为逐包发送时。应一起比较吞吐、负载延迟和内存。

每轮均记录随机 netem 丢包和网络命名空间内的 UDP 接收缓冲区错误。UDP socket 饱和时，即使没有注入丢包也可能丢包。单轮吞吐测试中最大的接收缓冲区丢包数为：rust-raknet (`send`) 207,572 个数据报、rust-raknet（批量接口） 327,373 个数据报、C KCP 370,281 个数据报、quic-go 0 个数据报。这些丢包属于实测负载的一部分。随机丢包和共享 CPU 调度会影响结果，应结合范围判断，不能把较小的中位数差异当作通用排名。

全部 445 次测量均完成回包校验：120 次单连接吞吐、25 次稀疏延迟、180 次并发吞吐和 120 次负载延迟。这批结果替换此前的表格，并非与旧版库进行严格控制条件的前后对比。

构建命令、接口参数和负载细节见 [benchmark README](examples/test_benchmark/README.md)、[C KCP 测试适配器](examples/test_benchmark/kcp/README.md) 和 [quic-go 测试适配器](examples/test_benchmark/quic/README.md)。

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
