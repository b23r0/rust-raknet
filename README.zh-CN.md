<div align="center">

<img src="assets/logo.png" alt="rust-raknet otter logo" width="180">

# rust-raknet

[English](README.md) | **简体中文**

**高性能 RakNet 协议的 Rust 实现。**

[![Build](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml/badge.svg)](https://github.com/b23r0/rust-raknet/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/rust-raknet)](https://crates.io/crates/rust-raknet)
[![Documentation](https://img.shields.io/docsrs/rust-raknet/latest)](https://docs.rs/rust-raknet/latest/rust_raknet/)
[![Wiki](https://img.shields.io/badge/Wiki-EN%20%2F%20中文-007C83?logo=github)](https://github.com/b23r0/rust-raknet/wiki)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2)](https://discord.gg/ZKtYMvDFN4)

[功能特性](#功能特性) · [Wiki](https://github.com/b23r0/rust-raknet/wiki) · [快速开始](#快速开始) · [基岩版反向代理](#minecraft-基岩版) · [性能测试](#性能测试) · [参与贡献](#参与贡献)

</div>

`rust-raknet` 为 Rust 应用提供基于 UDP 的可靠消息传输。它基于 Tokio，实现了 RakNet 的握手、确认、重传、排序和分片机制。你可以根据业务需要，选择不同的消息交付方式。

## 功能特性

- **五种交付模式。** 支持尽力传输、丢弃过时状态、可靠事件传输，以及按独立通道有序交付。
- **异步客户端和服务端。** 基于 Tokio 的连接、监听、接收连接、收发接口，支持 IPv4 和 IPv6 地址编码。
- **保留消息边界。** 接收完整应用消息，无需从字节流重建消息。较大的 `ReliableOrdered` 消息会自动分片和重组。
- **丢包恢复。** 支持 ACK/NACK 区间、选择性重传和 ACK 缺口快速重传，结合 RTT 估计调整重传计时，并对重复重传进行退避。
- **显式小包合并。** 用 `send_batch` 或 `send_bytes_batch` 将已就绪的小型 `ReliableOrdered` 消息装入标准 RakNet 数据报，无需等待凑包计时器。
- **拥有所有权的缓冲区转发。** `Bytes` 收发接口与发送、重传队列共享不可变载荷，减少代理转发中的复制。
- **有界队列和背压。** 发送记账、接收重排和分片重组均有限制；持续发送者会等待可用容量。
- **可选发送策略。** `send-policy` feature 提供每连接在途限制、可靠/不可靠队列预算和单次发送预算。需要显式启用，实测影响见[策略对性能的影响](#策略对性能的影响)。
- **服务端调优。** 可配置名义 MTU、accept 队列和 UDP 接收缓冲区；Linux 可选 socket 分片、接收批处理和空闲维护优化。
- **发现与生命周期。** 支持未连接 ping/pong、自定义 MOTD、查询对端 RakNet 版本，以及 flush 和关闭接口。
- **可运行示例。** 回显、发现、反向代理和 benchmark 程序统一放在 [`examples/`](examples) 中。
- **Rust 实现。** 使用 Rust 2024，采用 MIT 协议，支持 Linux、Windows、macOS 和 BSD。平台专用快速路径提供可移植的回退实现。

**环境要求：** Rust 1.85+、Tokio 1.38+。

## 文档

**[Wiki](https://github.com/b23r0/rust-raknet/wiki)** 包含入门、交付模式、配置、协议细节和可复现的性能测试方法。默认英文，每篇指南均提供中文切换入口。

[快速开始](https://github.com/b23r0/rust-raknet/wiki/Quick-Start) ·
[RakNet 协议参考](https://github.com/b23r0/rust-raknet/wiki/Protocol-Reference) ·
[发送策略](https://github.com/b23r0/rust-raknet/wiki/Send-Policy) ·
[性能评估方法](https://github.com/b23r0/rust-raknet/wiki/Benchmark-Methodology)

已发布 API 的详细说明见 [docs.rs](https://docs.rs/rust-raknet/latest/rust_raknet/)。协议参考同时说明本库的实现限制和报文格式，并不代表原始 RakNet SDK 的所有功能均已实现。

## 快速开始

```toml
[dependencies]
rust-raknet = "1.1.0"
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

### 可选发送策略

下方 benchmark 分别列出默认构建与显式配置 `send-policy` 的构建。

该接口从 1.1.0 起提供，请启用 `send-policy` feature：

```toml
rust-raknet = { version = "1.1.0", features = ["send-policy"] }
```

默认构建编译原有发送队列与反馈路径，不包含新策略的状态或检查。启用 `send-policy` 后，调用 `set_send_options` 才会为连接启用新策略。未配置的连接继续使用原有共享队列和 64 帧窗口，`send_options().await` 返回 `None`；配置后返回 `Some(options)`。

启用新策略后，不可靠消息使用独立待发送队列与预算，可靠窗口或常规可靠预算满时，小型实时消息仍可入队。混合发送按字节轮转处理可靠重传、可靠新消息与不可靠消息；未配置的连接保留原有发送和批量处理路径。调度器不会等待凑包，也不会替换或静默丢弃已接受的不可靠消息。

在客户端连接或服务端接受连接后、开始批量传输前配置：

```rust
use rust_raknet::SendOptions;

let options = SendOptions::default()
    .with_in_flight_limits(64, 256 * 1024)?
    .with_queue_budgets(256 * 1024, 64 * 1024)?
    .with_flush_budget(256 * 1024)?;
socket.set_send_options(options).await?;
```

在途字节按每个可靠帧的编码大小记账，包含分片及帧集合头；合包里的每帧分别计数。队列预算按消息数据和估算的帧元数据记账，不代表进程内存或网络带宽上限。可靠预算包含尚未确认的数据；一次较大的可靠发送（包括批量发送）可以超过软预算，但新接受的可靠数据必须在 64 MiB 总记账上限内为不可靠队列留下预留空间。调低配置不丢弃已有消息；旧数据排空后再按新限制接纳。

大窗口可能改善高 RTT 吞吐，也可能增加共享瓶颈的排队和丢包。此调度器不提供自适应拥塞控制或带宽比例保证。单次发送达到字节预算后，剩余工作由已有连接维护任务继续处理，不新增凑包定时器。自定义突发预算过小时，剩余帧可能等到下一次发送、ACK 或维护周期；如果不需要主动限制突发，保留默认预算。应用如需在可靠 `send` 等待容量时继续发送实时消息，应使用独立发送任务。

配置和未配置连接均保留原有快速补发与丢包反馈后的合包回退行为。

#### 策略对性能的影响

`send-policy` 控制队列接纳、可靠在途数据及发送突发，不是一个吞吐加速预设。默认构建不编译策略代码。启用 feature 但未调用 setter 的连接仍使用原有队列策略，不过并不是默认二进制；benchmark 的策略列在两端都显式调用了 setter。

benchmark 使用 **`SendOptions::default()`**，在途字节上限为 **64 MiB**。上方示例把它改成了 **256 KiB**，不能直接套用默认预设的实测结果。调用 `set_send_options(SendOptions::default())` 会启用策略，不会恢复到未配置的原有队列策略。

| 参数 | 可能的收益 | 需要评估的代价 |
| --- | --- | --- |
| 在途帧数／字节 | 限制未确认的可靠数据，减轻瓶颈队列压力 | 小窗口可能限制健康高 RTT 链路吞吐；大窗口可能增加排队和丢包 |
| 独立队列预算 | 可靠发送等待容量时，仍为不可靠更新预留空间 | 两类流量共享链路，更多不可靠接纳可能减少可靠带宽或增加网络丢包 |
| flush 字节预算 | 限制一次突发，让其他工作获得调度机会 | 小预算可能把剩余帧推迟到下一次发送、ACK 或维护 tick |

以下是 10 月 3 日使用默认策略预设的实测中位数；完整范围和内存见[性能表](#性能测试)。

| 可靠有序负载 | 未启用策略 | 显式配置策略 | 中位数变化 |
| --- | --- | --- | --- |
| 单连接，800 B，无注入丢包，普通吞吐 | 185.57 MiB/s | 175.19 MiB/s | −5.6% |
| 1024 连接，800 B，无注入丢包，普通吞吐 | 464.75 MiB/s | 486.67 MiB/s | +4.7% |
| 1024 连接，800 B，无注入丢包，批量吞吐 | 481.91 MiB/s | 423.69 MiB/s | −12.1% |
| 1024 连接，64 B，无注入丢包，普通发送负载下的 99% RTT | 54.03 ms | 100.53 ms | +86.1%（更慢） |

这些三轮测量体现取舍，不表示差异已具有统计显著性。策略记账和调度可能增加处理工作，也会改变消息接纳与突发时机；即使线协议不变，吞吐和较慢消息延迟仍可能变化。本组测量没有隔离出每项变化的单一原因。

另一组混合流量测试使用 8 条独立进程连接，共享从 100 Mbps／30 ms RTT 降到 2 Mbps／800 ms RTT 的链路。候选发送端使用默认策略预设。拥塞阶段，已收到不可靠消息中较慢 1% 的单程延迟从 1894.73 ms 降到 743.49 ms，但可靠吞吐从 0.072704 Mbps 降到 0.059392 Mbps；全程不可靠消息到达率从 68.65% 降到 60.01%。队列隔离不保证交付，也不能消除网络拥塞。

只有可靠流量时，先用默认构建。需要连接级限制或混合队列隔离时再启用策略，按实际消息大小、连接数和网络测试。混合流量使用独立发送任务，同时比较吞吐、负载延迟、到达率和内存。它不会自动适应拥塞，也不存在通用最优窗口。

客户端、accept 后的连接、代理两端及运行时修改限制的用法，见[发送策略 Wiki](https://github.com/b23r0/rust-raknet/wiki/Send-Policy)。

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

以下对比的是 **rust-raknet 的具体实现**，与 TCP、官方 C KCP 核心和 quic-go 对比。普通发送和批量接口分别测试默认构建与显式启用 `send-policy` 的配置。这些数据不代表 RakNet 协议本身的通用性能。

**测试日期：2026 年 10 月 3 日。** Intel Core i7-9700F（8 核）、Linux x86_64，内核 `7.0.11-76070011-generic`；Rust 1.98.1、Tokio 1.53.1、Go 1.27.1，优化构建。所有进程都运行在任务专属副本、独立缓存及私有回环网络命名空间中。回环 MTU 为 1500 B，GSO/GRO 限制为单包，netem 队列上限 100000 包，进程使用 `nice 10`。未修改主机网络设置。

### 对比实现与接口模式

| 实现／接口 | GitHub 地址 | 测试版本／配置 |
| --- | --- | --- |
| TCP | [TCP 回显驱动](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark)、[Tokio](https://github.com/tokio-rs/tokio) | 驱动 0.1.0；Tokio 1.53.1；上述 Linux TCP 栈 |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 1.0.0，基于 `7b72b25863b2b8370c0471b808a56e77019a05ce` 加本提交中的发送策略优化；默认构建 |
| rust-raknet（批量接口） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一默认库构建；显式调用批量接口 |
| rust-raknet (`send` + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一源码；启用 Cargo feature，客户端与服务端均调用 `set_send_options(SendOptions::default())` |
| rust-raknet（批量接口 + `send-policy`） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一策略构建与配置；显式调用批量接口 |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | 上游 C 核心原样；提交 `b1a7a2101dcbb96017681a500d6b82bbe5a88766` |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0；Go 1.27.1 |

- **普通发送**：两端使用 `send`／`recv`，不调用批量接口。
- **批量接口**：单连接客户端使用 `send_batch`，并发客户端使用 `send_bytes_batch`，回显服务端使用 `recv_bytes_batch`／`send_bytes_batch`。每次最多提交 16 条已准备好的消息，不等待凑批。
- **默认构建**：不编译 `send-policy`。策略列启用该 feature，客户端和服务端均调用 `set_send_options(SendOptions::default())`。

四列 rust-raknet 均使用 `ReliableOrdered` 和 64 帧可靠在途窗口。策略列另外使用 64 MiB 编码在途字节上限、256 KiB 可靠队列软预算、64 KiB 不可靠队列预留及 256 KiB 单次 flush 预算。UDP 批量接收与空闲维护关闭；消息合并和 UDP 系统调用批量发送是不同机制。

一个 UDP 数据报最多合并八条非分片消息，且受 MTU 限制。两条 800 B 消息无法合包，4096 B 消息需要分片。收到匹配的 NACK 或发生可靠重传超时后，连接后续合包关闭，包括预热阶段的反馈。**调用批量接口不等于每轮都实际合包。**

TCP 使用 `TCP_NODELAY`、四字节小端记录长度、完整记录写入及复用缓冲区。C KCP 使用消息模式、单连接收发窗口 64/64、并发窗口 64/128、`nodelay(1, 10, 2, 1)`，即时写入和 ACK，不启用 FEC 或加密。quic-go 每连接使用一条双向可靠有序流，保留 TLS 加密、拥塞控制与流控；这些实现的功能不完全相同。

吞吐按**每方向已验证的有效消息数据**计数，越高越好；RTT 越低越好。1 MiB = 1048576 B。吞吐和持续负载延迟各重复 3 轮，稀疏延迟重复 5 轮，七列运行顺序轮换，每次启动新服务端并建立新连接。随机丢包作用于两个方向的数据和 ACK。

表中数值取各轮中位数，括号为最小–最大范围，每项均带单位。**C = 客户端，S = 服务端。** 范围及峰值 RSS 直接列在对应实测表中。延迟分位数先按每轮计算，再取各轮对应分位数的中位数。

RSS 使用 Linux `wait4` 记录整进程峰值，覆盖启动、建连、预热、负载和退出，包含运行时、分配器、应用缓冲及延迟样本，不含内核 socket 缓冲。因此它不是完整网络内存或每连接内存；C/S 峰值也不一定同时发生。

### 单连接吞吐量

应用层最多保持 64 条待回显消息。每轮先做 100 次预热和 300 次顺序 RTT 采样，再开始吞吐计时。Rust／Go 使用 4 个 worker，C KCP 每进程一个事件循环。该组吞吐不绑定 CPU，不代表相同 CPU 使用效率。

| 网络场景 | 消息大小／突发数量 | TCP<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口）<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send` + `send-policy`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口 + `send-policy`）<br>吞吐量（范围）<br>峰值 RSS：C / S | C KCP<br>吞吐量（范围）<br>峰值 RSS：C / S | quic-go<br>吞吐量（范围）<br>峰值 RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 无注入丢包 | 800 B / 200,000 条消息 | 154.44 MiB/s (152.93–160.66 MiB/s)<br>C: 3.53 MiB (3.52–3.66 MiB)<br>S: 3.43 MiB (3.41–3.50 MiB) | 185.57 MiB/s (176.25–186.72 MiB/s)<br>C: 3.92 MiB (3.79–3.93 MiB)<br>S: 3.74 MiB (3.67–3.75 MiB) | 169.67 MiB/s (169.62–175.89 MiB/s)<br>C: 3.86 MiB (3.85–3.90 MiB)<br>S: 3.76 MiB (3.74–4.12 MiB) | 175.19 MiB/s (158.46–176.84 MiB/s)<br>C: 3.93 MiB (3.90–4.02 MiB)<br>S: 3.78 MiB (3.71–3.87 MiB) | 172.00 MiB/s (135.85–193.53 MiB/s)<br>C: 3.92 MiB (3.78–4.03 MiB)<br>S: 4.04 MiB (3.81–4.07 MiB) | 153.60 MiB/s (145.14–157.41 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 112.61 MiB/s (109.10–117.96 MiB/s)<br>C: 14.11 MiB (14.08–14.11 MiB)<br>S: 13.86 MiB (13.78–14.08 MiB) |
| 1% 丢包 | 800 B / 50,000 条消息 | 24.07 MiB/s (22.39–75.22 MiB/s)<br>C: 3.32 MiB (3.31–3.54 MiB)<br>S: 3.43 MiB (3.20–3.45 MiB) | 153.70 MiB/s (153.41–172.75 MiB/s)<br>C: 3.92 MiB (3.91–4.00 MiB)<br>S: 3.91 MiB (3.90–3.95 MiB) | 157.34 MiB/s (151.22–178.39 MiB/s)<br>C: 3.79 MiB (3.77–4.11 MiB)<br>S: 3.93 MiB (3.86–4.04 MiB) | 151.29 MiB/s (145.01–178.60 MiB/s)<br>C: 3.73 MiB (3.66–3.79 MiB)<br>S: 3.78 MiB (3.73–3.96 MiB) | 153.44 MiB/s (148.61–166.67 MiB/s)<br>C: 3.86 MiB (3.72–3.97 MiB)<br>S: 3.72 MiB (3.62–4.04 MiB) | 130.51 MiB/s (130.29–134.65 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) | 65.26 MiB/s (61.60–89.68 MiB/s)<br>C: 13.81 MiB (13.30–13.92 MiB)<br>S: 14.07 MiB (13.44–14.13 MiB) |
| 5% 丢包 | 800 B / 50,000 条消息 | 1.03 MiB/s (0.96–1.14 MiB/s)<br>C: 3.60 MiB (3.54–3.61 MiB)<br>S: 3.43 MiB (3.41–3.55 MiB) | 140.90 MiB/s (134.09–141.29 MiB/s)<br>C: 3.86 MiB (3.71–3.86 MiB)<br>S: 3.90 MiB (3.63–3.91 MiB) | 155.22 MiB/s (137.11–166.69 MiB/s)<br>C: 3.77 MiB (3.75–3.91 MiB)<br>S: 3.78 MiB (3.74–3.99 MiB) | 140.20 MiB/s (138.98–141.27 MiB/s)<br>C: 3.71 MiB (3.51–3.75 MiB)<br>S: 3.72 MiB (3.58–3.88 MiB) | 141.24 MiB/s (107.59–145.54 MiB/s)<br>C: 3.84 MiB (3.82–3.88 MiB)<br>S: 3.73 MiB (3.70–3.91 MiB) | 104.70 MiB/s (100.74–114.25 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 9.96 MiB/s (8.77–10.80 MiB/s)<br>C: 14.06 MiB (13.82–14.19 MiB)<br>S: 13.76 MiB (13.66–14.36 MiB) |
| 无注入丢包 | 64 B / 300,000 条消息 | 14.01 MiB/s (13.91–14.17 MiB/s)<br>C: 3.48 MiB (3.31–3.68 MiB)<br>S: 3.41 MiB (3.33–3.53 MiB) | 16.40 MiB/s (15.99–16.64 MiB/s)<br>C: 3.67 MiB (3.60–3.70 MiB)<br>S: 3.75 MiB (3.73–3.79 MiB) | 48.44 MiB/s (48.35–51.05 MiB/s)<br>C: 3.75 MiB (3.65–3.81 MiB)<br>S: 3.64 MiB (3.59–3.79 MiB) | 16.59 MiB/s (16.15–16.63 MiB/s)<br>C: 3.66 MiB (3.57–3.68 MiB)<br>S: 3.80 MiB (3.53–3.98 MiB) | 49.40 MiB/s (47.83–51.00 MiB/s)<br>C: 3.71 MiB (3.57–3.89 MiB)<br>S: 3.72 MiB (3.55–3.73 MiB) | 12.33 MiB/s (12.03–12.48 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 11.16 MiB/s (10.08–19.35 MiB/s)<br>C: 13.57 MiB (13.57–13.57 MiB)<br>S: 13.19 MiB (12.94–13.69 MiB) |
| 1% 丢包 | 64 B / 100,000 条消息 | 2.49 MiB/s (2.02–2.54 MiB/s)<br>C: 3.71 MiB (3.70–3.75 MiB)<br>S: 3.40 MiB (3.38–3.54 MiB) | 14.40 MiB/s (14.28–14.79 MiB/s)<br>C: 3.73 MiB (3.67–3.77 MiB)<br>S: 3.55 MiB (3.49–3.75 MiB) | 16.06 MiB/s (15.09–16.07 MiB/s)<br>C: 3.68 MiB (3.62–3.79 MiB)<br>S: 3.71 MiB (3.56–3.76 MiB) | 16.38 MiB/s (14.46–16.54 MiB/s)<br>C: 3.81 MiB (3.68–3.86 MiB)<br>S: 3.72 MiB (3.66–3.74 MiB) | 14.63 MiB/s (14.40–14.98 MiB/s)<br>C: 3.75 MiB (3.57–3.88 MiB)<br>S: 3.65 MiB (3.62–3.89 MiB) | 11.25 MiB/s (10.75–11.48 MiB/s)<br>C: 1.88 MiB (1.88–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 23.94 MiB/s (11.01–26.12 MiB/s)<br>C: 12.88 MiB (12.44–13.07 MiB)<br>S: 12.07 MiB (11.69–12.07 MiB) |
| 5% 丢包 | 64 B / 100,000 条消息 | 0.13 MiB/s (0.13–0.15 MiB/s)<br>C: 3.55 MiB (3.52–3.70 MiB)<br>S: 3.38 MiB (3.20–3.44 MiB) | 13.73 MiB/s (13.53–13.91 MiB/s)<br>C: 3.70 MiB (3.63–3.88 MiB)<br>S: 3.59 MiB (3.47–3.67 MiB) | 13.48 MiB/s (10.60–13.62 MiB/s)<br>C: 3.71 MiB (3.67–3.74 MiB)<br>S: 3.78 MiB (3.68–3.82 MiB) | 13.42 MiB/s (13.42–13.51 MiB/s)<br>C: 3.65 MiB (3.64–3.69 MiB)<br>S: 3.63 MiB (3.61–3.95 MiB) | 13.45 MiB/s (13.11–13.77 MiB/s)<br>C: 3.66 MiB (3.60–3.80 MiB)<br>S: 3.79 MiB (3.77–3.80 MiB) | 10.01 MiB/s (9.91–10.64 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 3.59 MiB/s (2.81–3.69 MiB/s)<br>C: 12.76 MiB (12.69–13.07 MiB)<br>S: 11.94 MiB (11.94–12.00 MiB) |
| 无注入丢包 | 4,096 B / 50,000 条消息 | 346.57 MiB/s (344.74–351.15 MiB/s)<br>C: 3.58 MiB (3.55–3.66 MiB)<br>S: 3.40 MiB (3.39–3.46 MiB) | 303.25 MiB/s (279.28–312.47 MiB/s)<br>C: 4.77 MiB (4.36–4.98 MiB)<br>S: 5.10 MiB (4.91–5.29 MiB) | 309.56 MiB/s (306.58–313.50 MiB/s)<br>C: 4.90 MiB (4.45–5.11 MiB)<br>S: 4.80 MiB (4.77–5.01 MiB) | 310.12 MiB/s (307.68–312.53 MiB/s)<br>C: 4.49 MiB (4.43–5.10 MiB)<br>S: 4.99 MiB (4.83–5.08 MiB) | 313.04 MiB/s (310.98–313.37 MiB/s)<br>C: 4.70 MiB (4.61–4.77 MiB)<br>S: 4.95 MiB (4.79–4.95 MiB) | 253.51 MiB/s (249.69–257.45 MiB/s)<br>C: 1.86 MiB (1.86–1.87 MiB)<br>S: 1.87 MiB (1.75–1.87 MiB) | 220.97 MiB/s (80.24–226.14 MiB/s)<br>C: 14.09 MiB (13.90–14.12 MiB)<br>S: 14.00 MiB (13.65–14.14 MiB) |
| 1% 丢包 + 每方向 5 ms 延迟 | 800 B / 3,000 条消息 | 1.08 MiB/s (1.04–1.13 MiB/s)<br>C: 3.66 MiB (3.51–3.75 MiB)<br>S: 3.43 MiB (3.41–3.56 MiB) | 2.95 MiB/s (2.80–3.09 MiB/s)<br>C: 3.76 MiB (3.53–3.78 MiB)<br>S: 3.72 MiB (3.71–3.86 MiB) | 3.01 MiB/s (2.90–3.03 MiB/s)<br>C: 3.86 MiB (3.69–3.98 MiB)<br>S: 3.85 MiB (3.62–3.88 MiB) | 3.09 MiB/s (2.89–3.39 MiB/s)<br>C: 3.76 MiB (3.69–3.85 MiB)<br>S: 3.76 MiB (3.60–3.90 MiB) | 2.91 MiB/s (2.88–3.02 MiB/s)<br>C: 3.81 MiB (3.62–3.86 MiB)<br>S: 3.75 MiB (3.55–3.75 MiB) | 2.88 MiB/s (2.86–3.24 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 1.78 MiB/s (1.62–1.81 MiB/s)<br>C: 11.57 MiB (11.32–11.63 MiB)<br>S: 11.69 MiB (11.57–11.69 MiB) |

单连接 rust-raknet 使用 1428 B 标称 MTU（含 28 B IPv4/UDP 开销），C KCP 与 quic-go 使用 1400 B UDP 数据报预算。4096 B 消息在 rust-raknet 与 KCP 中触发分片。

### 稀疏请求的往返延迟

800 B 消息，无注入丢包；每轮 1000 次预热、10000 个顺序回显样本，共 5 轮。客户端和服务端绑定不同 CPU，Rust／Go 每进程 4 个 worker，C KCP 一个事件循环。该组逐条发送并等待回显，批量接口每次只传入一条消息，不合包。

| 实现／接口 | RTT 中位数（范围） | 95% RTT（范围） | 99% RTT（范围） | 峰值 RSS：C / S（范围） |
| --- | --- | --- | --- | --- |
| TCP | 12.80 µs (12.70–13.00 µs) | 19.50 µs (18.90–21.20 µs) | 40.60 µs (38.60–42.40 µs) | C: 3.88 MiB (3.88–4.02 MiB)<br>S: 3.56 MiB (3.47–3.60 MiB) |
| rust-raknet (`send`) | 17.40 µs (17.30–18.20 µs) | 30.90 µs (25.90–31.30 µs) | 49.50 µs (48.50–51.30 µs) | C: 4.25 MiB (4.15–4.32 MiB)<br>S: 4.07 MiB (3.95–4.39 MiB) |
| rust-raknet（批量接口） | 17.80 µs (17.50–18.70 µs) | 31.00 µs (24.10–32.50 µs) | 50.10 µs (49.00–58.60 µs) | C: 4.22 MiB (4.18–4.35 MiB)<br>S: 4.07 MiB (3.96–4.24 MiB) |
| rust-raknet (`send` + `send-policy`) | 17.90 µs (17.40–18.00 µs) | 30.30 µs (26.00–32.30 µs) | 49.40 µs (48.10–53.30 µs) | C: 4.26 MiB (4.20–4.38 MiB)<br>S: 4.10 MiB (4.00–4.21 MiB) |
| rust-raknet（批量接口 + `send-policy`） | 17.80 µs (17.40–18.90 µs) | 31.50 µs (27.40–32.20 µs) | 50.50 µs (46.60–53.20 µs) | C: 4.21 MiB (4.04–4.25 MiB)<br>S: 4.12 MiB (4.04–4.20 MiB) |
| C KCP | 10.30 µs (10.10–10.40 µs) | 15.80 µs (13.10–16.10 µs) | 34.80 µs (34.30–35.00 µs) | C: 1.88 MiB (1.87–2.00 MiB)<br>S: 1.87 MiB (1.75–1.88 MiB) |
| quic-go | 62.50 µs (62.30–62.80 µs) | 102.10 µs (99.90–103.10 µs) | 146.10 µs (143.90–152.10 µs) | C: 13.25 MiB (12.94–13.44 MiB)<br>S: 10.94 MiB (10.94–11.06 MiB) |

这些是稀疏接口路径延迟，不是突发交付延迟。并发吞吐输出的预热 RTT 也不作为下方持续负载延迟。

### 高并发吞吐量

每轮验证 1048576 条有序突发回显，每连接应用窗口 16 条。连接编号、消息编号、完整消息数据和顺序均校验。每连接先做 20 次顺序回显，再通过共同起跑屏障开始突发；建连和预热不计入吞吐时间。连接保留到全部突发完成。

七列均使用 4 个 worker，服务端使用 4 个监听／接收 socket 和 `SO_REUSEPORT`，服务端绑定 CPU 0–3，客户端绑定 CPU 4–7。CPU 绑定不代表独占 CPU。

| 消息大小 | 连接数 | 注入丢包 | TCP<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口）<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send` + `send-policy`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口 + `send-policy`）<br>吞吐量（范围）<br>峰值 RSS：C / S | C KCP<br>吞吐量（范围）<br>峰值 RSS：C / S | quic-go<br>吞吐量（范围）<br>峰值 RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 条连接 | 0% | 647.10 MiB/s (618.06–652.91 MiB/s)<br>C: 3.89 MiB (3.86–3.96 MiB)<br>S: 3.32 MiB (3.24–3.55 MiB) | 572.91 MiB/s (539.19–595.28 MiB/s)<br>C: 7.13 MiB (6.96–7.16 MiB)<br>S: 7.58 MiB (7.43–7.89 MiB) | 599.30 MiB/s (538.39–601.84 MiB/s)<br>C: 7.25 MiB (7.12–7.29 MiB)<br>S: 7.89 MiB (7.77–7.98 MiB) | 544.68 MiB/s (522.42–572.92 MiB/s)<br>C: 6.98 MiB (6.86–7.32 MiB)<br>S: 7.55 MiB (7.45–7.77 MiB) | 572.00 MiB/s (556.00–572.14 MiB/s)<br>C: 7.38 MiB (7.25–7.45 MiB)<br>S: 7.82 MiB (7.62–7.91 MiB) | 386.44 MiB/s (375.77–405.71 MiB/s)<br>C: 2.88 MiB (2.88–3.25 MiB)<br>S: 3.13 MiB (3.13–3.38 MiB) | 226.60 MiB/s (217.63–228.88 MiB/s)<br>C: 30.00 MiB (29.82–30.17 MiB)<br>S: 34.61 MiB (32.57–35.63 MiB) |
| 800 B | 256 条连接 | 0% | 590.00 MiB/s (504.74–591.30 MiB/s)<br>C: 5.76 MiB (5.68–5.77 MiB)<br>S: 3.62 MiB (3.45–3.76 MiB) | 567.45 MiB/s (541.52–573.54 MiB/s)<br>C: 15.67 MiB (15.66–16.02 MiB)<br>S: 14.22 MiB (14.05–14.38 MiB) | 544.23 MiB/s (534.40–562.60 MiB/s)<br>C: 16.43 MiB (16.24–16.64 MiB)<br>S: 14.94 MiB (14.79–15.10 MiB) | 569.72 MiB/s (507.60–570.51 MiB/s)<br>C: 15.79 MiB (15.64–16.14 MiB)<br>S: 14.40 MiB (14.21–14.53 MiB) | 524.99 MiB/s (513.03–527.06 MiB/s)<br>C: 16.95 MiB (16.61–16.99 MiB)<br>S: 14.85 MiB (14.07–15.06 MiB) | 396.53 MiB/s (335.64–405.05 MiB/s)<br>C: 6.50 MiB (6.38–6.50 MiB)<br>S: 6.88 MiB (6.75–7.00 MiB) | 189.10 MiB/s (188.33–189.23 MiB/s)<br>C: 77.04 MiB (76.49–79.95 MiB)<br>S: 86.39 MiB (75.70–87.51 MiB) |
| 800 B | 1,024 条连接 | 0% | 570.54 MiB/s (569.63–581.21 MiB/s)<br>C: 11.45 MiB (10.53–12.27 MiB)<br>S: 4.81 MiB (4.66–4.99 MiB) | 464.75 MiB/s (461.92–554.91 MiB/s)<br>C: 47.22 MiB (47.02–49.01 MiB)<br>S: 39.73 MiB (39.58–43.51 MiB) | 481.91 MiB/s (452.22–505.51 MiB/s)<br>C: 48.61 MiB (47.86–49.06 MiB)<br>S: 42.52 MiB (41.35–43.19 MiB) | 486.67 MiB/s (463.14–486.71 MiB/s)<br>C: 46.94 MiB (46.79–49.04 MiB)<br>S: 41.18 MiB (39.42–41.68 MiB) | 423.69 MiB/s (361.22–521.99 MiB/s)<br>C: 48.64 MiB (48.37–49.91 MiB)<br>S: 42.30 MiB (39.68–44.45 MiB) | 369.82 MiB/s (343.27–383.97 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 20.63 MiB (20.63–20.88 MiB) | 161.86 MiB/s (154.55–168.66 MiB/s)<br>C: 242.18 MiB (240.76–247.57 MiB)<br>S: 242.51 MiB (198.42–273.20 MiB) |
| 800 B | 2,048 条连接 | 0% | 569.08 MiB/s (553.16–570.47 MiB/s)<br>C: 18.93 MiB (16.64–20.12 MiB)<br>S: 6.61 MiB (6.46–6.67 MiB) | 395.73 MiB/s (370.16–429.25 MiB/s)<br>C: 87.11 MiB (86.32–88.48 MiB)<br>S: 70.56 MiB (69.97–71.50 MiB) | 374.79 MiB/s (373.14–377.43 MiB/s)<br>C: 89.93 MiB (87.41–91.23 MiB)<br>S: 73.80 MiB (71.18–74.89 MiB) | 418.58 MiB/s (413.37–425.69 MiB/s)<br>C: 86.88 MiB (86.30–87.59 MiB)<br>S: 71.92 MiB (68.51–73.05 MiB) | 384.77 MiB/s (382.97–399.33 MiB/s)<br>C: 89.42 MiB (88.61–89.62 MiB)<br>S: 73.54 MiB (72.30–74.41 MiB) | 325.00 MiB/s (314.37–356.58 MiB/s)<br>C: 38.88 MiB (38.75–38.88 MiB)<br>S: 38.88 MiB (38.63–38.88 MiB) | 117.58 MiB/s (117.50–141.56 MiB/s)<br>C: 473.72 MiB (469.05–529.84 MiB)<br>S: 416.27 MiB (387.32–471.65 MiB) |
| 800 B | 64 条连接 | 1% | 453.95 MiB/s (415.34–470.16 MiB/s)<br>C: 4.07 MiB (4.02–4.23 MiB)<br>S: 3.44 MiB (3.44–3.50 MiB) | 515.14 MiB/s (488.61–521.14 MiB/s)<br>C: 7.24 MiB (7.10–7.75 MiB)<br>S: 7.60 MiB (7.52–8.20 MiB) | 536.13 MiB/s (523.38–558.63 MiB/s)<br>C: 7.49 MiB (7.32–7.64 MiB)<br>S: 7.76 MiB (7.57–7.87 MiB) | 523.12 MiB/s (466.22–540.41 MiB/s)<br>C: 7.46 MiB (7.25–7.47 MiB)<br>S: 7.63 MiB (7.54–8.09 MiB) | 502.00 MiB/s (457.08–542.14 MiB/s)<br>C: 7.61 MiB (7.39–7.66 MiB)<br>S: 7.87 MiB (7.76–8.07 MiB) | 369.95 MiB/s (360.65–384.65 MiB/s)<br>C: 2.88 MiB (2.75–3.00 MiB)<br>S: 3.25 MiB (3.25–3.25 MiB) | 218.94 MiB/s (203.58–221.61 MiB/s)<br>C: 28.92 MiB (28.11–29.08 MiB)<br>S: 31.34 MiB (31.06–37.63 MiB) |
| 800 B | 256 条连接 | 1% | 524.45 MiB/s (440.81–562.14 MiB/s)<br>C: 5.64 MiB (5.39–5.69 MiB)<br>S: 3.62 MiB (3.52–3.73 MiB) | 481.38 MiB/s (439.64–487.96 MiB/s)<br>C: 16.76 MiB (15.92–17.03 MiB)<br>S: 15.76 MiB (15.14–15.84 MiB) | 482.40 MiB/s (470.57–499.85 MiB/s)<br>C: 17.21 MiB (16.81–17.70 MiB)<br>S: 15.55 MiB (15.06–16.17 MiB) | 506.93 MiB/s (455.77–508.54 MiB/s)<br>C: 16.34 MiB (16.01–17.27 MiB)<br>S: 15.27 MiB (14.77–15.50 MiB) | 495.02 MiB/s (451.15–513.80 MiB/s)<br>C: 17.05 MiB (16.89–17.21 MiB)<br>S: 15.91 MiB (15.67–15.95 MiB) | 397.44 MiB/s (391.17–423.06 MiB/s)<br>C: 6.38 MiB (6.38–6.50 MiB)<br>S: 6.75 MiB (6.75–6.88 MiB) | 174.82 MiB/s (174.44–181.90 MiB/s)<br>C: 75.01 MiB (73.70–76.02 MiB)<br>S: 92.40 MiB (77.64–95.14 MiB) |
| 800 B | 1,024 条连接 | 1% | 477.11 MiB/s (421.92–496.74 MiB/s)<br>C: 12.33 MiB (12.24–12.44 MiB)<br>S: 4.82 MiB (4.68–4.95 MiB) | 450.82 MiB/s (436.15–481.93 MiB/s)<br>C: 48.47 MiB (48.41–48.57 MiB)<br>S: 41.77 MiB (41.41–42.66 MiB) | 426.79 MiB/s (424.29–444.82 MiB/s)<br>C: 50.86 MiB (49.43–52.42 MiB)<br>S: 43.78 MiB (42.70–43.86 MiB) | 399.41 MiB/s (396.93–439.76 MiB/s)<br>C: 49.58 MiB (46.70–51.19 MiB)<br>S: 42.44 MiB (42.13–43.79 MiB) | 435.19 MiB/s (425.02–444.45 MiB/s)<br>C: 50.48 MiB (50.29–51.38 MiB)<br>S: 43.03 MiB (42.79–44.09 MiB) | 360.22 MiB/s (347.16–361.17 MiB/s)<br>C: 20.25 MiB (20.25–20.25 MiB)<br>S: 19.88 MiB (19.50–20.50 MiB) | 98.72 MiB/s (94.11–126.29 MiB/s)<br>C: 304.08 MiB (256.50–333.60 MiB)<br>S: 203.75 MiB (202.14–270.56 MiB) |
| 800 B | 2,048 条连接 | 1% | 433.33 MiB/s (319.12–468.32 MiB/s)<br>C: 20.67 MiB (19.20–21.36 MiB)<br>S: 6.61 MiB (6.52–6.62 MiB) | 370.29 MiB/s (343.56–370.62 MiB/s)<br>C: 87.20 MiB (87.10–89.61 MiB)<br>S: 73.31 MiB (71.26–75.51 MiB) | 365.95 MiB/s (337.58–378.87 MiB/s)<br>C: 92.91 MiB (92.22–93.15 MiB)<br>S: 76.68 MiB (75.34–80.20 MiB) | 352.13 MiB/s (342.57–364.23 MiB/s)<br>C: 89.61 MiB (88.52–91.56 MiB)<br>S: 75.65 MiB (73.05–78.55 MiB) | 361.54 MiB/s (333.15–374.76 MiB/s)<br>C: 93.25 MiB (91.78–93.64 MiB)<br>S: 77.25 MiB (76.32–77.80 MiB) | 310.78 MiB/s (294.50–331.89 MiB/s)<br>C: 38.88 MiB (38.88–38.88 MiB)<br>S: 37.25 MiB (36.88–37.38 MiB) | 36.77 MiB/s (31.45–54.96 MiB/s)<br>C: 744.62 MiB (708.05–756.38 MiB)<br>S: 405.50 MiB (393.24–422.23 MiB) |
| 64 B | 64 条连接 | 0% | 66.86 MiB/s (62.89–67.40 MiB/s)<br>C: 3.95 MiB (3.75–4.17 MiB)<br>S: 3.30 MiB (3.25–3.39 MiB) | 48.08 MiB/s (45.28–48.43 MiB/s)<br>C: 5.73 MiB (5.68–5.89 MiB)<br>S: 5.86 MiB (5.81–6.00 MiB) | 106.70 MiB/s (105.15–121.63 MiB/s)<br>C: 5.75 MiB (5.69–5.97 MiB)<br>S: 6.20 MiB (6.16–6.26 MiB) | 47.19 MiB/s (46.29–47.47 MiB/s)<br>C: 5.66 MiB (5.62–5.83 MiB)<br>S: 5.88 MiB (5.87–6.05 MiB) | 112.17 MiB/s (110.42–114.05 MiB/s)<br>C: 5.77 MiB (5.59–5.98 MiB)<br>S: 6.20 MiB (5.95–6.24 MiB) | 32.94 MiB/s (31.28–35.36 MiB/s)<br>C: 2.38 MiB (2.25–2.38 MiB)<br>S: 2.50 MiB (2.38–2.50 MiB) | 138.04 MiB/s (130.66–138.30 MiB/s)<br>C: 21.69 MiB (21.46–22.21 MiB)<br>S: 19.14 MiB (18.86–19.57 MiB) |
| 64 B | 1,024 条连接 | 0% | 54.18 MiB/s (53.17–54.75 MiB/s)<br>C: 9.54 MiB (9.14–9.59 MiB)<br>S: 4.04 MiB (3.93–4.27 MiB) | 46.73 MiB/s (43.62–47.39 MiB/s)<br>C: 32.80 MiB (32.34–33.24 MiB)<br>S: 27.71 MiB (27.59–29.36 MiB) | 142.95 MiB/s (132.99–143.79 MiB/s)<br>C: 31.85 MiB (31.79–32.16 MiB)<br>S: 25.89 MiB (25.87–26.07 MiB) | 48.69 MiB/s (45.51–48.92 MiB/s)<br>C: 32.62 MiB (32.01–32.73 MiB)<br>S: 26.83 MiB (26.44–27.57 MiB) | 144.11 MiB/s (141.29–147.03 MiB/s)<br>C: 32.29 MiB (32.00–32.37 MiB)<br>S: 26.27 MiB (26.03–26.36 MiB) | 44.69 MiB/s (44.14–44.78 MiB/s)<br>C: 8.63 MiB (8.63–8.88 MiB)<br>S: 9.38 MiB (9.38–9.38 MiB) | 107.37 MiB/s (103.80–107.93 MiB/s)<br>C: 138.57 MiB (133.51–140.70 MiB)<br>S: 94.24 MiB (92.34–97.74 MiB) |
| 64 B | 64 条连接 | 1% | 43.37 MiB/s (38.98–45.56 MiB/s)<br>C: 3.85 MiB (3.72–3.93 MiB)<br>S: 3.36 MiB (3.25–3.37 MiB) | 45.08 MiB/s (43.00–46.26 MiB/s)<br>C: 5.82 MiB (5.81–5.96 MiB)<br>S: 6.12 MiB (6.04–6.16 MiB) | 48.32 MiB/s (45.90–49.52 MiB/s)<br>C: 6.00 MiB (5.94–6.21 MiB)<br>S: 6.39 MiB (6.39–6.43 MiB) | 45.22 MiB/s (44.37–46.15 MiB/s)<br>C: 5.84 MiB (5.77–5.89 MiB)<br>S: 6.05 MiB (6.04–6.29 MiB) | 46.12 MiB/s (44.19–49.77 MiB/s)<br>C: 5.84 MiB (5.83–5.84 MiB)<br>S: 6.39 MiB (6.39–6.49 MiB) | 33.61 MiB/s (32.28–34.08 MiB/s)<br>C: 2.38 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.25–2.38 MiB) | 56.68 MiB/s (55.71–58.67 MiB/s)<br>C: 21.63 MiB (21.57–22.01 MiB)<br>S: 18.76 MiB (18.54–19.51 MiB) |
| 64 B | 1,024 条连接 | 1% | 49.42 MiB/s (44.09–49.50 MiB/s)<br>C: 9.49 MiB (9.04–9.56 MiB)<br>S: 4.19 MiB (4.18–4.20 MiB) | 42.22 MiB/s (41.98–42.53 MiB/s)<br>C: 34.29 MiB (34.18–35.00 MiB)<br>S: 29.17 MiB (28.34–29.84 MiB) | 40.23 MiB/s (37.99–50.13 MiB/s)<br>C: 36.50 MiB (36.12–37.07 MiB)<br>S: 30.35 MiB (30.16–30.81 MiB) | 41.11 MiB/s (40.40–43.16 MiB/s)<br>C: 35.39 MiB (35.20–35.49 MiB)<br>S: 29.26 MiB (28.94–30.04 MiB) | 46.76 MiB/s (44.89–48.70 MiB/s)<br>C: 35.22 MiB (34.18–35.80 MiB)<br>S: 30.81 MiB (30.25–31.41 MiB) | 40.65 MiB/s (38.83–40.95 MiB/s)<br>C: 8.88 MiB (8.75–8.88 MiB)<br>S: 9.25 MiB (9.12–9.25 MiB) | 76.18 MiB/s (74.54–78.11 MiB/s)<br>C: 137.88 MiB (137.87–143.32 MiB)<br>S: 98.14 MiB (93.57–98.76 MiB) |

并发 rust-raknet 使用默认 1400 B 标称 MTU，quic-go 使用 1372 B UDP 数据报预算并关闭路径 MTU 探测。C KCP 使用 1400 B UDP MTU，物理预算多 28 B；本组 64 B／800 B 消息在三者中均不分片。

### 持续负载下的交付延迟

并发客户端使用 `--loaded-rtt`，发送前打时间戳，在消费已验证回显时计算 RTT，贯穿突发全过程，包含客户端和服务端排队。每轮验证 262144 条突发回显，保持相同的 16 条应用窗口、worker、CPU 绑定和服务端配置，各场景 3 轮。

延迟采样与吞吐表分开运行，不把该组带采样的吞吐混入上方吞吐表。RSS 包括客户端保存、汇总和排序的 RTT 样本。

| 消息大小 | 连接数 | 注入丢包 | TCP<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet (`send`)<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口）<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet (`send` + `send-policy`)<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口 + `send-policy`）<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | C KCP<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | quic-go<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 条连接 | 0% | 50%: 0.91 ms (0.91–0.97 ms)<br>99%: 5.29 ms (4.16–5.79 ms)<br>C: 11.46 MiB (11.45–11.65 MiB)<br>S: 3.41 MiB (3.39–3.46 MiB) | 50%: 1.02 ms (0.94–1.11 ms)<br>99%: 3.00 ms (2.92–7.79 ms)<br>C: 13.38 MiB (13.34–13.43 MiB)<br>S: 5.73 MiB (5.53–5.80 MiB) | 50%: 0.47 ms (0.44–0.49 ms)<br>99%: 4.45 ms (3.81–4.86 ms)<br>C: 13.44 MiB (13.32–13.48 MiB)<br>S: 5.89 MiB (5.59–6.05 MiB) | 50%: 1.05 ms (1.00–1.08 ms)<br>99%: 3.68 ms (3.08–4.17 ms)<br>C: 13.46 MiB (13.43–13.57 MiB)<br>S: 5.82 MiB (5.69–5.88 MiB) | 50%: 0.41 ms (0.36–0.45 ms)<br>99%: 1.63 ms (1.52–2.11 ms)<br>C: 13.44 MiB (13.29–13.66 MiB)<br>S: 6.05 MiB (6.05–6.17 MiB) | 50%: 1.37 ms (1.30–1.43 ms)<br>99%: 5.92 ms (5.19–6.45 ms)<br>C: 6.21 MiB (6.09–6.38 MiB)<br>S: 2.38 MiB (2.25–2.50 MiB) | 50%: 0.34 ms (0.33–0.35 ms)<br>99%: 2.15 ms (1.86–2.98 ms)<br>C: 31.70 MiB (31.02–34.45 MiB)<br>S: 17.19 MiB (17.14–17.20 MiB) |
| 64 B | 1,024 条连接 | 0% | 50%: 17.00 ms (16.35–20.55 ms)<br>99%: 38.89 ms (35.81–41.34 ms)<br>C: 16.45 MiB (16.40–17.46 MiB)<br>S: 4.03 MiB (3.91–4.09 MiB) | 50%: 17.64 ms (17.44–17.82 ms)<br>99%: 54.03 ms (33.19–85.12 ms)<br>C: 36.50 MiB (36.35–36.61 MiB)<br>S: 23.05 MiB (22.97–23.55 MiB) | 50%: 5.92 ms (5.17–7.24 ms)<br>99%: 17.86 ms (16.15–19.01 ms)<br>C: 38.12 MiB (38.12–38.15 MiB)<br>S: 24.21 MiB (24.21–24.46 MiB) | 50%: 16.03 ms (15.72–16.09 ms)<br>99%: 100.53 ms (98.57–105.22 ms)<br>C: 36.65 MiB (36.41–36.68 MiB)<br>S: 24.65 MiB (24.40–24.88 MiB) | 50%: 4.77 ms (4.54–5.04 ms)<br>99%: 19.28 ms (16.26–22.82 ms)<br>C: 38.04 MiB (37.96–38.27 MiB)<br>S: 24.68 MiB (24.47–24.76 MiB) | 50%: 6.93 ms (6.85–8.06 ms)<br>99%: 108.46 ms (107.70–110.22 ms)<br>C: 10.82 MiB (10.69–10.94 MiB)<br>S: 9.25 MiB (9.25–9.25 MiB) | 50%: 9.09 ms (8.77–9.91 ms)<br>99%: 20.50 ms (15.12–24.04 ms)<br>C: 135.82 MiB (135.01–139.26 MiB)<br>S: 91.45 MiB (90.82–92.95 MiB) |
| 64 B | 64 条连接 | 1% | 50%: 0.66 ms (0.65–0.68 ms)<br>99%: 6.63 ms (3.90–6.67 ms)<br>C: 11.78 MiB (11.59–11.87 MiB)<br>S: 3.33 MiB (3.20–3.48 MiB) | 50%: 0.93 ms (0.93–1.06 ms)<br>99%: 5.95 ms (5.68–6.34 ms)<br>C: 13.67 MiB (13.63–13.74 MiB)<br>S: 5.96 MiB (5.74–6.07 MiB) | 50%: 0.90 ms (0.88–0.93 ms)<br>99%: 4.69 ms (4.45–4.93 ms)<br>C: 13.71 MiB (13.54–13.73 MiB)<br>S: 6.17 MiB (6.16–6.18 MiB) | 50%: 0.97 ms (0.92–1.00 ms)<br>99%: 5.37 ms (5.21–5.81 ms)<br>C: 13.61 MiB (13.57–13.67 MiB)<br>S: 6.01 MiB (5.99–6.03 MiB) | 50%: 0.96 ms (0.91–1.04 ms)<br>99%: 5.05 ms (3.96–5.24 ms)<br>C: 13.79 MiB (13.77–13.83 MiB)<br>S: 6.12 MiB (6.06–6.13 MiB) | 50%: 1.26 ms (1.20–1.38 ms)<br>99%: 7.14 ms (5.69–8.50 ms)<br>C: 6.20 MiB (6.19–6.28 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.11 ms (0.08–0.12 ms)<br>99%: 27.27 ms (27.18–27.30 ms)<br>C: 31.87 MiB (30.76–33.14 MiB)<br>S: 17.39 MiB (17.10–17.88 MiB) |
| 64 B | 1,024 条连接 | 1% | 50%: 16.74 ms (16.20–17.32 ms)<br>99%: 35.64 ms (33.99–40.71 ms)<br>C: 17.28 MiB (16.58–17.44 MiB)<br>S: 4.02 MiB (4.01–4.11 MiB) | 50%: 16.33 ms (16.03–17.67 ms)<br>99%: 89.30 ms (69.20–94.48 ms)<br>C: 37.62 MiB (35.47–37.67 MiB)<br>S: 24.96 MiB (24.45–25.02 MiB) | 50%: 12.30 ms (11.03–13.47 ms)<br>99%: 48.59 ms (38.75–68.93 ms)<br>C: 38.56 MiB (38.27–38.65 MiB)<br>S: 25.69 MiB (25.44–25.82 MiB) | 50%: 16.92 ms (16.90–17.35 ms)<br>99%: 58.16 ms (53.54–81.28 ms)<br>C: 37.74 MiB (36.85–38.20 MiB)<br>S: 25.46 MiB (25.38–25.70 MiB) | 50%: 12.38 ms (10.43–13.56 ms)<br>99%: 46.75 ms (38.17–52.41 ms)<br>C: 37.84 MiB (37.37–38.78 MiB)<br>S: 26.14 MiB (26.06–26.26 MiB) | 50%: 10.39 ms (7.30–15.26 ms)<br>99%: 127.16 ms (118.57–136.22 ms)<br>C: 10.69 MiB (10.69–10.82 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 50%: 8.95 ms (8.53–10.60 ms)<br>99%: 65.93 ms (48.74–71.05 ms)<br>C: 140.26 MiB (134.89–142.89 MiB)<br>S: 89.75 MiB (88.20–90.76 MiB) |
| 800 B | 64 条连接 | 0% | 50%: 1.16 ms (1.14–1.19 ms)<br>99%: 5.62 ms (3.61–6.01 ms)<br>C: 11.75 MiB (11.74–11.97 MiB)<br>S: 3.32 MiB (3.30–3.34 MiB) | 50%: 1.26 ms (1.17–1.39 ms)<br>99%: 8.01 ms (7.68–8.59 ms)<br>C: 14.76 MiB (14.64–14.84 MiB)<br>S: 7.20 MiB (7.12–7.63 MiB) | 50%: 1.16 ms (1.16–1.26 ms)<br>99%: 7.08 ms (5.26–7.72 ms)<br>C: 15.10 MiB (14.85–15.12 MiB)<br>S: 7.45 MiB (7.28–7.63 MiB) | 50%: 1.12 ms (1.11–1.15 ms)<br>99%: 4.91 ms (4.41–9.43 ms)<br>C: 14.67 MiB (14.52–14.77 MiB)<br>S: 7.17 MiB (7.07–7.71 MiB) | 50%: 1.23 ms (1.18–1.23 ms)<br>99%: 4.90 ms (3.50–7.48 ms)<br>C: 14.95 MiB (14.83–14.97 MiB)<br>S: 7.33 MiB (7.21–7.42 MiB) | 50%: 1.10 ms (1.06–1.35 ms)<br>99%: 23.70 ms (17.13–30.84 ms)<br>C: 6.95 MiB (6.78–6.97 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 50%: 3.09 ms (3.03–3.10 ms)<br>99%: 9.96 ms (9.08–10.89 ms)<br>C: 38.99 MiB (36.68–40.48 MiB)<br>S: 30.52 MiB (30.01–33.09 MiB) |
| 800 B | 1,024 条连接 | 0% | 50%: 21.41 ms (20.62–26.02 ms)<br>99%: 40.69 ms (40.26–88.40 ms)<br>C: 20.19 MiB (19.71–20.35 MiB)<br>S: 4.96 MiB (4.82–5.03 MiB) | 50%: 16.63 ms (12.34–17.05 ms)<br>99%: 119.40 ms (114.39–160.62 ms)<br>C: 50.56 MiB (50.16–50.70 MiB)<br>S: 34.45 MiB (34.35–34.50 MiB) | 50%: 13.61 ms (12.15–21.93 ms)<br>99%: 187.28 ms (42.55–206.00 ms)<br>C: 50.86 MiB (50.79–52.08 MiB)<br>S: 36.60 MiB (35.46–37.78 MiB) | 50%: 17.78 ms (16.31–18.46 ms)<br>99%: 118.04 ms (109.44–131.81 ms)<br>C: 50.73 MiB (50.50–51.07 MiB)<br>S: 35.01 MiB (34.98–36.59 MiB) | 50%: 15.19 ms (14.80–19.37 ms)<br>99%: 122.03 ms (95.69–207.62 ms)<br>C: 52.02 MiB (51.84–52.64 MiB)<br>S: 36.10 MiB (35.41–36.42 MiB) | 50%: 17.59 ms (16.12–17.80 ms)<br>99%: 189.30 ms (122.63–215.00 ms)<br>C: 22.19 MiB (22.07–22.19 MiB)<br>S: 20.13 MiB (19.50–20.75 MiB) | 50%: 69.40 ms (68.71–74.07 ms)<br>99%: 187.23 ms (126.03–233.35 ms)<br>C: 231.64 MiB (224.52–241.82 MiB)<br>S: 164.76 MiB (162.76–168.14 MiB) |
| 800 B | 64 条连接 | 1% | 50%: 0.69 ms (0.56–0.77 ms)<br>99%: 6.64 ms (4.28–7.94 ms)<br>C: 11.75 MiB (11.73–11.98 MiB)<br>S: 3.51 MiB (3.22–3.58 MiB) | 50%: 1.06 ms (1.04–1.15 ms)<br>99%: 5.84 ms (5.10–6.58 ms)<br>C: 14.89 MiB (14.81–15.00 MiB)<br>S: 7.45 MiB (7.28–7.46 MiB) | 50%: 1.08 ms (0.97–1.12 ms)<br>99%: 5.62 ms (3.66–5.70 ms)<br>C: 15.17 MiB (15.04–15.23 MiB)<br>S: 7.57 MiB (7.54–7.94 MiB) | 50%: 1.07 ms (1.00–1.09 ms)<br>99%: 6.14 ms (5.55–6.87 ms)<br>C: 14.91 MiB (14.86–15.12 MiB)<br>S: 7.34 MiB (7.29–7.88 MiB) | 50%: 1.11 ms (1.04–1.34 ms)<br>99%: 5.11 ms (4.99–6.43 ms)<br>C: 15.18 MiB (15.09–15.33 MiB)<br>S: 7.68 MiB (7.27–8.07 MiB) | 50%: 1.15 ms (1.11–1.21 ms)<br>99%: 16.06 ms (11.78–31.16 ms)<br>C: 6.84 MiB (6.66–6.86 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.13 ms (3.13–3.19 ms)<br>99%: 19.22 ms (18.83–20.43 ms)<br>C: 40.45 MiB (36.31–42.92 MiB)<br>S: 29.07 MiB (28.70–32.08 MiB) |
| 800 B | 1,024 条连接 | 1% | 50%: 20.62 ms (18.69–25.53 ms)<br>99%: 53.56 ms (50.23–148.66 ms)<br>C: 18.92 MiB (18.27–20.41 MiB)<br>S: 4.86 MiB (4.81–4.89 MiB) | 50%: 19.83 ms (11.15–21.07 ms)<br>99%: 99.99 ms (98.24–177.44 ms)<br>C: 51.28 MiB (49.68–51.31 MiB)<br>S: 36.79 MiB (35.71–37.23 MiB) | 50%: 14.21 ms (11.76–25.77 ms)<br>99%: 132.55 ms (117.70–177.69 ms)<br>C: 51.98 MiB (50.08–55.20 MiB)<br>S: 37.01 MiB (36.27–38.05 MiB) | 50%: 18.70 ms (13.54–19.96 ms)<br>99%: 113.76 ms (100.94–172.58 ms)<br>C: 50.99 MiB (49.09–51.76 MiB)<br>S: 36.54 MiB (35.43–37.35 MiB) | 50%: 19.49 ms (17.25–21.00 ms)<br>99%: 113.99 ms (84.43–120.12 ms)<br>C: 50.91 MiB (50.61–53.35 MiB)<br>S: 37.81 MiB (37.49–38.38 MiB) | 50%: 18.19 ms (17.60–21.47 ms)<br>99%: 124.72 ms (114.22–353.38 ms)<br>C: 22.32 MiB (22.07–22.32 MiB)<br>S: 20.13 MiB (19.50–20.50 MiB) | 50%: 67.66 ms (65.72–68.10 ms)<br>99%: 289.12 ms (282.72–302.63 ms)<br>C: 245.05 MiB (243.28–248.14 MiB)<br>S: 183.63 MiB (180.06–195.50 MiB) |

### 如何理解这些结果

- 1024 连接、64 B、无注入丢包时，默认普通发送为 **46.73 MiB/s**，批量接口为 **142.95 MiB/s**；TCP、C KCP、quic-go 分别为 **54.18、44.69、107.37 MiB/s**。
- 单连接 800 B、5% 丢包时，默认普通发送为 **140.90 MiB/s**；TCP、C KCP、quic-go 分别为 **1.03、104.70、9.96 MiB/s**。这反映的是本组配置下的可靠有序回显负载。
- 策略并非所有场景都更快。1024 连接、800 B、无注入丢包时，普通发送从 **464.75** 到 **486.67 MiB/s**，批量接口从 **481.91** 到 **423.69 MiB/s**。
- 1024 连接、64 B、无注入丢包的持续负载中，普通发送启用策略后典型 RTT 从 **17.64 ms** 到 **16.03 ms**，99% RTT 却从 **54.03 ms** 到 **100.53 ms**。应同时看吞吐、较慢消息延迟和内存。

本轮全部使用可靠有序流量，没有测试混合可靠／不可靠调度、带宽骤降或拥塞公平性。`send-policy` 保持可选启用；本组不能代替混合流量验证，也不是与旧版本严格控制变量的前后对比。

无注入丢包也可能发生 UDP 接收缓冲溢出。每轮记录私有网络的 UDP 错误计数与 qdisc；短负载、随机丢包和 CPU 调度会造成波动，小差距不能据此认定为统计显著或普适排名。

单次吞吐测试的最大命名空间 UDP 接收缓冲溢出计数：rust-raknet (`send`)：175,509 个数据报；rust-raknet（批量接口）：407,363 个数据报；rust-raknet (`send` + `send-policy`)：287,825 个数据报；rust-raknet（批量接口 + `send-policy`）：295,212 个数据报；C KCP：417,267 个数据报；quic-go：7,000 个数据报。

全部 **623 次测量**通过回显校验：单连接吞吐 168 次、稀疏延迟 35 次、并发吞吐 252 次、持续负载延迟 168 次。

默认驱动、接口选项与工作负载说明见 [benchmark README](examples/test_benchmark/README.md)、[C KCP 驱动](examples/test_benchmark/kcp/README.md)和 [quic-go 驱动](examples/test_benchmark/quic/README.md)。策略列在独立验证副本中构建，新增的调用只用于在每个已连接或接受的 socket 上应用 `SendOptions::default()`。

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
