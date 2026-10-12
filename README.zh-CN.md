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

[功能特性](#功能特性) · [Wiki](https://github.com/b23r0/rust-raknet/wiki) · [快速开始](#快速开始) · [性能测试](#性能测试) · [参与贡献](#参与贡献)

</div>

`rust-raknet` 是一个Rust实现的高性能Reliable UDP实时消息传输协议异步网络库。

## 功能特性

- **五种交付模式。** 支持尽力传输、丢弃过时状态、可靠事件传输，以及按独立通道有序交付。
- **异步客户端和服务端。** 基于 Tokio 的连接、监听、接收连接、收发接口，支持 IPv4 和 IPv6 地址编码。
- **同步客户端和服务端。** 可选 `blocking` feature 提供同步连接、监听、接收连接、收发及超时接口，多个连接共享 Tokio runtime；调用间隙仍持续处理 ACK 和重传。[使用方法](#同步-api)。
- **保留消息边界。** 接收完整应用消息，无需从字节流重建消息。较大的 `ReliableOrdered` 消息会自动分片和重组。
- **丢包恢复。** 支持 ACK/NACK 区间、选择性重传和 ACK 缺口快速重传，结合 RTT 估计调整重传计时，并对重复重传进行退避。
- **尾包丢失探测。** 可选 `recovery-policy` 设置可缩短孤立尾包丢失后的等待，完整数据报探测保留正常 RTO 回退，也可单独启用按期限重试。[设置与性能取舍](https://github.com/b23r0/rust-raknet/wiki/Recovery-Policy)。
- **拥有所有权的缓冲区转发。** `Bytes` 收发接口与发送、重传队列共享不可变载荷，减少代理转发中的复制。
- **有界队列和背压。** 发送记账、接收重排和分片重组均有限制；持续发送者会等待可用容量。
- **可选发送策略。** `send-policy` feature 提供每连接在途限制、可靠/不可靠队列预算和单次发送预算。需要显式启用，实测影响见[策略对性能的影响](#策略对性能的影响)。
- **服务端调优。** 可配置名义 MTU、accept 队列和 UDP 接收缓冲区；Linux 可选 socket 分片、接收批处理和空闲维护优化。
- **发现与生命周期。** 支持未连接 ping/pong、自定义 MOTD、查询对端 RakNet 版本，以及 flush 和关闭接口。
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
rust-raknet = "1.4.0"
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

### 同步 API

可选 `blocking` feature 提供同步包装。创建一个 `blocking::RaknetRuntime`，
供多个客户端和监听器共享。调用间隙，网络工作线程仍持续处理 ACK、重传和心跳，
不会为每条连接创建 runtime。默认使用一个工作线程，可通过
`with_worker_threads` 调整。

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

服务端使用 `runtime.bind(...)`，完成监听配置后调用 `listen()`，再使用
`accept()` 或 `accept_timeout()` 接收连接。同步 socket 也支持拥有所有权的缓冲区、
批量接口、排序通道，以及可选发送与恢复策略。

这些方法会阻塞调用线程，不应在游戏更新系统或 Tokio 异步上下文中使用；
后者返回 `blocking::Error::AsyncContext`。`send` 不等待对端确认，`flush` 才等待。
发送超时后，已进入队列的数据仍可能发送；批量发送超时也可能已发送部分消息，
重试可能造成重复。需要确认交付时应显式调用 `flush` 和 `close`；
最后一个 runtime 所有者被释放后停止后台工作，不等待 ACK。

吞吐、延迟、内存及共享 runtime 的开销见独立的[blocking 性能测试](examples/test_benchmark/BLOCKING.md)。

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
rust-raknet = { version = "1.4.0", features = ["send-policy"] }
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

以下是 10 月 12 日使用默认策略预设的实测中位数；完整范围和内存见[性能表](#性能测试)。

| 可靠有序负载 | 未启用策略 | 显式配置策略 | 中位数变化 |
| --- | --- | --- | --- |
| 单连接，800 B，无注入丢包，普通吞吐 | 186.77 MiB/s | 183.84 MiB/s | -1.6% |
| 1024 连接，800 B，无注入丢包，普通吞吐 | 372.54 MiB/s | 395.28 MiB/s | +6.1% |
| 1024 连接，800 B，无注入丢包，批量吞吐 | 413.36 MiB/s | 381.62 MiB/s | -7.7% |
| 1024 连接，64 B，无注入丢包，普通发送负载下的 99% RTT | 94.65 ms | 101.42 ms | +7.1% |

这些三轮测量体现取舍，不表示差异已具有统计显著性。策略记账和调度可能增加处理工作，也会改变消息接纳与突发时机；即使线协议不变，吞吐和较慢消息延迟仍可能变化。本组测量没有隔离出每项变化的单一原因。

此前 10 月 3 日的混合流量测试使用 8 条独立进程连接，共享从 100 Mbps／30 ms RTT 降到 2 Mbps／800 ms RTT 的链路。候选发送端使用默认策略预设。拥塞阶段，已收到不可靠消息中较慢 1% 的单程延迟从 1894.73 ms 降到 743.49 ms，但可靠吞吐从 0.072704 Mbps 降到 0.059392 Mbps；全程不可靠消息到达率从 68.65% 降到 60.01%。队列隔离不保证交付，也不能消除网络拥塞。

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
包含 `Cargo.toml` 的子目录是独立的 Cargo 项目，
使用 `cargo run --manifest-path examples/PROJECT/Cargo.toml` 运行。

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

## 恢复策略

启用 `recovery-policy` 后，为每个已连接或已接受的 socket 配置：

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

所有选项默认关闭。探测隐含启用按期限维护，等待至少为设置下限与 1.5 倍平滑 RTT 中的较大值，并保留原 RTO 期限；10 ms 不是交付时间保证。提前恢复可以缩短孤立尾包丢失的等待，但可能降低持续吞吐或增加拥塞延迟。下方 benchmark 测量 **1.4.0**，未开启 recovery-policy。

[恢复策略指南](https://github.com/b23r0/rust-raknet/wiki/Recovery-Policy)介绍仅期限重试、RTT 进展退避、关闭选项和实测取舍；[配置参数参考](https://github.com/b23r0/rust-raknet/wiki/Configuration-Reference)列出全部公开调优参数。

## 性能测试

以下对比的是 **rust-raknet 的具体实现**，与 TCP、官方 C KCP 核心和 quic-go 对比。普通发送和批量接口分别测试默认构建与显式启用 `send-policy` 的配置。这些数据不代表 RakNet 协议本身的通用性能。

**测试日期：2026 年 10 月 12 日。** Intel Core i7-9700F（8 核）、Linux x86_64，内核 `7.0.11-76070011-generic`；Rust 1.98.1、Tokio 1.53.2、Go 1.27.2，优化构建。所有进程都运行在任务专属副本、独立缓存及私有回环网络命名空间中。回环 MTU 为 1500 B，GSO/GRO 限制为单包，netem 队列上限 100000 包，进程使用 `nice 10`。未修改主机网络设置。

### 对比实现与接口模式

| 实现／接口 | GitHub 地址 | 测试版本／配置 |
| --- | --- | --- |
| TCP | [TCP 回显驱动](https://github.com/b23r0/rust-raknet/tree/main/examples/test_benchmark)、[Tokio](https://github.com/tokio-rs/tokio) | 驱动 0.1.0；Tokio 1.53.2；上述 Linux TCP 栈 |
| rust-raknet (`send`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 1.4.0；源码提交 `1199ac7`；默认构建 |
| rust-raknet（批量接口） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一默认库构建；显式调用批量接口 |
| rust-raknet (`send` + `send-policy`) | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一源码；启用 Cargo feature，客户端与服务端均调用 `set_send_options(SendOptions::default())` |
| rust-raknet（批量接口 + `send-policy`） | [b23r0/rust-raknet](https://github.com/b23r0/rust-raknet) | 同一策略构建与配置；显式调用批量接口 |
| C KCP | [skywind3000/kcp](https://github.com/skywind3000/kcp/tree/b1a7a2101dcbb96017681a500d6b82bbe5a88766) | 上游 C 核心原样；提交 `b1a7a2101dcbb96017681a500d6b82bbe5a88766` |
| quic-go | [quic-go/quic-go](https://github.com/quic-go/quic-go/tree/v0.63.0) | v0.63.0；Go 1.27.2 |

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
| 无注入丢包 | 800 B / 200,000 条消息 | 155.70 MiB/s (153.20–158.29 MiB/s)<br>C: 3.47 MiB (3.20–3.63 MiB)<br>S: 3.40 MiB (3.32–3.42 MiB) | 186.77 MiB/s (185.18–187.22 MiB/s)<br>C: 3.70 MiB (3.62–3.88 MiB)<br>S: 3.73 MiB (3.72–4.06 MiB) | 188.15 MiB/s (183.18–192.66 MiB/s)<br>C: 3.86 MiB (3.71–3.92 MiB)<br>S: 3.93 MiB (3.82–4.03 MiB) | 183.84 MiB/s (179.94–184.51 MiB/s)<br>C: 3.85 MiB (3.82–4.00 MiB)<br>S: 3.85 MiB (3.81–3.88 MiB) | 189.79 MiB/s (188.38–189.79 MiB/s)<br>C: 3.96 MiB (3.80–4.00 MiB)<br>S: 3.82 MiB (3.77–3.84 MiB) | 151.63 MiB/s (146.52–153.23 MiB/s)<br>C: 1.75 MiB (1.75–1.88 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 115.12 MiB/s (111.58–116.03 MiB/s)<br>C: 13.97 MiB (13.93–14.16 MiB)<br>S: 13.96 MiB (13.72–14.06 MiB) |
| 1% 丢包 | 800 B / 50,000 条消息 | 17.81 MiB/s (11.76–25.83 MiB/s)<br>C: 3.70 MiB (3.61–3.74 MiB)<br>S: 3.49 MiB (3.45–3.51 MiB) | 167.31 MiB/s (153.82–168.05 MiB/s)<br>C: 3.89 MiB (3.58–4.10 MiB)<br>S: 3.90 MiB (3.79–3.93 MiB) | 158.95 MiB/s (125.90–164.43 MiB/s)<br>C: 3.80 MiB (3.71–4.02 MiB)<br>S: 3.68 MiB (3.61–3.73 MiB) | 155.70 MiB/s (150.78–158.95 MiB/s)<br>C: 3.82 MiB (3.78–3.87 MiB)<br>S: 3.88 MiB (3.66–4.06 MiB) | 167.31 MiB/s (159.61–174.19 MiB/s)<br>C: 3.70 MiB (3.69–4.09 MiB)<br>S: 3.96 MiB (3.90–4.00 MiB) | 131.80 MiB/s (126.00–142.75 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 44.67 MiB/s (39.12–75.04 MiB/s)<br>C: 13.70 MiB (13.20–13.95 MiB)<br>S: 13.51 MiB (13.38–13.58 MiB) |
| 5% 丢包 | 800 B / 50,000 条消息 | 1.03 MiB/s (1.01–1.20 MiB/s)<br>C: 3.57 MiB (3.54–3.63 MiB)<br>S: 3.48 MiB (3.36–3.50 MiB) | 146.72 MiB/s (146.16–156.98 MiB/s)<br>C: 3.68 MiB (3.59–3.79 MiB)<br>S: 4.01 MiB (3.73–4.03 MiB) | 106.85 MiB/s (49.10–141.29 MiB/s)<br>C: 3.92 MiB (3.66–3.92 MiB)<br>S: 3.76 MiB (3.73–3.86 MiB) | 155.70 MiB/s (110.25–160.28 MiB/s)<br>C: 3.98 MiB (3.92–3.98 MiB)<br>S: 3.78 MiB (3.64–3.96 MiB) | 145.05 MiB/s (143.41–151.38 MiB/s)<br>C: 3.76 MiB (3.71–3.96 MiB)<br>S: 3.88 MiB (3.76–4.08 MiB) | 102.06 MiB/s (95.89–107.54 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 8.07 MiB/s (8.00–8.96 MiB/s)<br>C: 13.92 MiB (13.90–14.17 MiB)<br>S: 13.99 MiB (13.45–14.01 MiB) |
| 无注入丢包 | 64 B / 300,000 条消息 | 14.36 MiB/s (14.25–14.54 MiB/s)<br>C: 3.63 MiB (3.52–3.66 MiB)<br>S: 3.40 MiB (3.29–3.48 MiB) | 15.94 MiB/s (15.76–15.95 MiB/s)<br>C: 3.68 MiB (3.63–3.74 MiB)<br>S: 3.73 MiB (3.61–3.74 MiB) | 47.07 MiB/s (20.95–50.03 MiB/s)<br>C: 3.66 MiB (3.66–3.79 MiB)<br>S: 3.79 MiB (3.55–3.85 MiB) | 15.10 MiB/s (15.01–15.73 MiB/s)<br>C: 3.73 MiB (3.71–3.75 MiB)<br>S: 3.75 MiB (3.69–3.93 MiB) | 27.96 MiB/s (17.73–49.49 MiB/s)<br>C: 3.82 MiB (3.71–3.89 MiB)<br>S: 3.79 MiB (3.60–3.80 MiB) | 12.13 MiB/s (11.66–12.28 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.88 MiB (1.75–1.88 MiB) | 29.02 MiB/s (18.59–30.44 MiB/s)<br>C: 13.76 MiB (13.70–13.77 MiB)<br>S: 13.33 MiB (13.13–13.38 MiB) |
| 1% 丢包 | 64 B / 100,000 条消息 | 2.62 MiB/s (2.26–2.62 MiB/s)<br>C: 3.50 MiB (3.42–3.52 MiB)<br>S: 3.28 MiB (3.16–3.41 MiB) | 14.60 MiB/s (14.06–15.65 MiB/s)<br>C: 3.80 MiB (3.68–3.83 MiB)<br>S: 3.64 MiB (3.62–3.71 MiB) | 15.30 MiB/s (14.40–15.85 MiB/s)<br>C: 3.67 MiB (3.67–3.67 MiB)<br>S: 3.76 MiB (3.69–3.78 MiB) | 14.85 MiB/s (14.64–15.11 MiB/s)<br>C: 3.71 MiB (3.70–3.79 MiB)<br>S: 3.82 MiB (3.62–3.91 MiB) | 14.96 MiB/s (10.63–15.03 MiB/s)<br>C: 3.82 MiB (3.71–3.90 MiB)<br>S: 3.78 MiB (3.73–3.82 MiB) | 11.39 MiB/s (11.16–11.55 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 24.55 MiB/s (10.66–24.69 MiB/s)<br>C: 13.07 MiB (12.07–13.07 MiB)<br>S: 11.82 MiB (11.63–11.88 MiB) |
| 5% 丢包 | 64 B / 100,000 条消息 | 0.13 MiB/s (0.12–0.14 MiB/s)<br>C: 3.48 MiB (3.46–3.78 MiB)<br>S: 3.48 MiB (3.32–3.48 MiB) | 13.56 MiB/s (12.88–14.00 MiB/s)<br>C: 3.66 MiB (3.60–3.71 MiB)<br>S: 3.74 MiB (3.59–3.77 MiB) | 13.38 MiB/s (13.13–14.00 MiB/s)<br>C: 3.66 MiB (3.48–3.69 MiB)<br>S: 3.78 MiB (3.73–3.91 MiB) | 13.62 MiB/s (10.78–13.72 MiB/s)<br>C: 3.73 MiB (3.54–3.87 MiB)<br>S: 3.68 MiB (3.66–3.70 MiB) | 13.53 MiB/s (12.01–13.56 MiB/s)<br>C: 3.65 MiB (3.62–3.72 MiB)<br>S: 3.80 MiB (3.65–3.85 MiB) | 9.85 MiB/s (9.84–10.02 MiB/s)<br>C: 1.88 MiB (1.75–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 4.91 MiB/s (4.26–5.26 MiB/s)<br>C: 12.63 MiB (12.45–12.70 MiB)<br>S: 12.13 MiB (12.07–12.25 MiB) |
| 无注入丢包 | 4,096 B / 50,000 条消息 | 350.65 MiB/s (350.02–352.55 MiB/s)<br>C: 3.60 MiB (3.56–3.67 MiB)<br>S: 3.50 MiB (3.48–3.53 MiB) | 282.24 MiB/s (279.02–285.54 MiB/s)<br>C: 4.81 MiB (4.37–4.89 MiB)<br>S: 4.94 MiB (4.93–5.02 MiB) | 283.88 MiB/s (105.98–286.38 MiB/s)<br>C: 4.66 MiB (4.66–4.95 MiB)<br>S: 5.03 MiB (4.81–5.21 MiB) | 282.24 MiB/s (281.03–287.22 MiB/s)<br>C: 4.90 MiB (4.84–4.94 MiB)<br>S: 5.07 MiB (4.92–5.14 MiB) | 286.38 MiB/s (164.40–290.21 MiB/s)<br>C: 5.03 MiB (4.88–5.04 MiB)<br>S: 5.08 MiB (4.87–5.12 MiB) | 239.75 MiB/s (223.99–249.66 MiB/s)<br>C: 1.86 MiB (1.86–1.88 MiB)<br>S: 1.75 MiB (1.75–1.88 MiB) | 212.51 MiB/s (210.72–215.61 MiB/s)<br>C: 14.21 MiB (13.78–14.42 MiB)<br>S: 13.75 MiB (13.67–13.98 MiB) |
| 10 ms RTT + 1% 丢包 | 800 B / 3,000 条消息 | 1.27 MiB/s (1.06–1.35 MiB/s)<br>C: 3.69 MiB (3.66–3.77 MiB)<br>S: 3.52 MiB (3.38–3.55 MiB) | 3.21 MiB/s (3.15–3.28 MiB/s)<br>C: 3.78 MiB (3.61–3.81 MiB)<br>S: 3.77 MiB (3.71–3.91 MiB) | 2.89 MiB/s (2.78–2.90 MiB/s)<br>C: 3.78 MiB (3.75–3.94 MiB)<br>S: 3.66 MiB (3.58–3.70 MiB) | 3.04 MiB/s (2.76–3.25 MiB/s)<br>C: 3.82 MiB (3.76–3.87 MiB)<br>S: 3.81 MiB (3.81–3.93 MiB) | 2.95 MiB/s (2.90–3.18 MiB/s)<br>C: 3.82 MiB (3.69–3.82 MiB)<br>S: 3.67 MiB (3.49–3.69 MiB) | 2.96 MiB/s (2.89–3.15 MiB/s)<br>C: 1.75 MiB (1.75–1.75 MiB)<br>S: 1.75 MiB (1.75–1.75 MiB) | 1.78 MiB/s (1.66–1.97 MiB/s)<br>C: 11.70 MiB (11.32–11.70 MiB)<br>S: 11.38 MiB (11.13–11.88 MiB) |

单连接 rust-raknet 使用 1428 B 标称 MTU（含 28 B IPv4/UDP 开销），C KCP 与 quic-go 使用 1400 B UDP 数据报预算。4096 B 消息在 rust-raknet 与 KCP 中触发分片。

### 稀疏请求的往返延迟

800 B 消息，无注入丢包；每轮 1000 次预热、10000 个顺序回显样本，共 5 轮。客户端和服务端绑定不同 CPU，Rust／Go 每进程 4 个 worker，C KCP 一个事件循环。该组逐条发送并等待回显，批量接口每次只传入一条消息，不合包。

| 实现／接口 | RTT 中位数（范围） | 95% RTT（范围） | 99% RTT（范围） | 峰值 RSS：C / S（范围） |
| --- | --- | --- | --- | --- |
| TCP | 13.00 µs (12.60–13.30 µs) | 26.80 µs (18.40–29.70 µs) | 66.10 µs (33.20–150.10 µs) | C: 3.91 MiB (3.87–3.93 MiB)<br>S: 3.56 MiB (3.49–3.67 MiB) |
| rust-raknet (`send`) | 17.90 µs (17.30–18.10 µs) | 33.70 µs (30.20–35.70 µs) | 73.10 µs (46.30–264.40 µs) | C: 4.24 MiB (4.12–4.31 MiB)<br>S: 4.02 MiB (3.98–4.29 MiB) |
| rust-raknet（批量接口） | 18.60 µs (17.50–18.80 µs) | 33.10 µs (31.40–36.70 µs) | 75.50 µs (47.30–111.20 µs) | C: 4.31 MiB (4.21–4.34 MiB)<br>S: 4.10 MiB (3.98–4.21 MiB) |
| rust-raknet (`send` + `send-policy`) | 17.40 µs (17.10–17.60 µs) | 31.40 µs (30.10–34.70 µs) | 48.60 µs (43.60–110.00 µs) | C: 4.32 MiB (4.09–4.39 MiB)<br>S: 4.03 MiB (3.96–4.05 MiB) |
| rust-raknet（批量接口 + `send-policy`） | 17.50 µs (17.20–18.30 µs) | 32.40 µs (21.80–35.50 µs) | 50.10 µs (46.30–205.50 µs) | C: 4.27 MiB (4.24–4.33 MiB)<br>S: 4.03 MiB (3.93–4.14 MiB) |
| C KCP | 10.20 µs (10.10–10.30 µs) | 24.00 µs (18.10–24.40 µs) | 36.20 µs (33.40–66.60 µs) | C: 1.87 MiB (1.87–1.88 MiB)<br>S: 1.87 MiB (1.75–1.99 MiB) |
| quic-go | 62.50 µs (62.30–63.90 µs) | 108.30 µs (97.30–141.70 µs) | 211.40 µs (126.90–449.30 µs) | C: 13.45 MiB (13.20–13.45 MiB)<br>S: 10.94 MiB (10.88–11.13 MiB) |

这些是稀疏接口路径延迟，不是突发交付延迟。并发吞吐输出的预热 RTT 也不作为下方持续负载延迟。

### 高并发吞吐量

每轮验证 1048576 条有序突发回显，每连接应用窗口 16 条。连接编号、消息编号、完整消息数据和顺序均校验。每连接先做 20 次顺序回显，再通过共同起跑屏障开始突发；建连和预热不计入吞吐时间。连接保留到全部突发完成。

七列均使用 4 个 worker，服务端使用 4 个监听／接收 socket 和 `SO_REUSEPORT`，服务端绑定 CPU 0–3，客户端绑定 CPU 4–7。CPU 绑定不代表独占 CPU。

| 消息大小 | 连接数 | 注入丢包 | TCP<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口）<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet (`send` + `send-policy`)<br>吞吐量（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口 + `send-policy`）<br>吞吐量（范围）<br>峰值 RSS：C / S | C KCP<br>吞吐量（范围）<br>峰值 RSS：C / S | quic-go<br>吞吐量（范围）<br>峰值 RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 800 B | 64 条连接 | 0% | 556.05 MiB/s (410.64–642.81 MiB/s)<br>C: 4.07 MiB (4.06–4.29 MiB)<br>S: 3.42 MiB (3.23–3.54 MiB) | 432.99 MiB/s (385.15–515.15 MiB/s)<br>C: 7.41 MiB (7.39–7.44 MiB)<br>S: 7.78 MiB (7.75–8.02 MiB) | 533.52 MiB/s (405.64–566.27 MiB/s)<br>C: 7.41 MiB (7.34–7.91 MiB)<br>S: 7.91 MiB (7.72–8.25 MiB) | 548.45 MiB/s (464.52–549.28 MiB/s)<br>C: 7.06 MiB (6.88–7.13 MiB)<br>S: 7.81 MiB (7.43–7.86 MiB) | 511.69 MiB/s (347.19–540.66 MiB/s)<br>C: 7.56 MiB (7.39–7.69 MiB)<br>S: 8.07 MiB (7.77–8.41 MiB) | 369.80 MiB/s (363.04–387.54 MiB/s)<br>C: 3.00 MiB (2.88–3.25 MiB)<br>S: 3.25 MiB (3.13–3.25 MiB) | 213.74 MiB/s (180.33–215.28 MiB/s)<br>C: 29.08 MiB (28.82–29.53 MiB)<br>S: 39.49 MiB (34.58–42.39 MiB) |
| 800 B | 256 条连接 | 0% | 538.72 MiB/s (537.62–568.30 MiB/s)<br>C: 5.82 MiB (4.90–5.95 MiB)<br>S: 3.70 MiB (3.43–3.80 MiB) | 437.67 MiB/s (357.31–440.04 MiB/s)<br>C: 15.65 MiB (15.27–15.83 MiB)<br>S: 14.26 MiB (14.16–14.64 MiB) | 445.78 MiB/s (436.02–468.97 MiB/s)<br>C: 16.65 MiB (16.59–16.74 MiB)<br>S: 14.96 MiB (14.85–15.03 MiB) | 441.06 MiB/s (439.33–519.85 MiB/s)<br>C: 15.70 MiB (15.68–15.97 MiB)<br>S: 14.41 MiB (13.92–14.97 MiB) | 455.89 MiB/s (415.13–469.25 MiB/s)<br>C: 16.51 MiB (16.14–17.27 MiB)<br>S: 15.01 MiB (14.52–15.35 MiB) | 345.96 MiB/s (330.21–358.24 MiB/s)<br>C: 6.38 MiB (6.38–6.50 MiB)<br>S: 6.88 MiB (6.75–7.00 MiB) | 148.73 MiB/s (147.87–170.36 MiB/s)<br>C: 77.93 MiB (75.91–82.06 MiB)<br>S: 109.27 MiB (100.62–116.70 MiB) |
| 800 B | 1,024 条连接 | 0% | 467.22 MiB/s (455.60–525.70 MiB/s)<br>C: 11.78 MiB (9.52–12.20 MiB)<br>S: 4.89 MiB (4.58–4.89 MiB) | 372.54 MiB/s (347.08–487.01 MiB/s)<br>C: 46.31 MiB (45.80–46.84 MiB)<br>S: 41.92 MiB (40.86–41.95 MiB) | 413.36 MiB/s (394.91–441.03 MiB/s)<br>C: 48.41 MiB (47.44–48.72 MiB)<br>S: 42.28 MiB (41.79–42.38 MiB) | 395.28 MiB/s (333.46–423.21 MiB/s)<br>C: 46.93 MiB (46.00–46.96 MiB)<br>S: 40.87 MiB (39.96–42.48 MiB) | 381.62 MiB/s (362.67–429.23 MiB/s)<br>C: 48.21 MiB (48.11–48.66 MiB)<br>S: 42.17 MiB (41.21–44.30 MiB) | 286.75 MiB/s (268.24–327.30 MiB/s)<br>C: 20.25 MiB (20.25–20.38 MiB)<br>S: 21.00 MiB (20.88–21.00 MiB) | 141.31 MiB/s (136.13–152.38 MiB/s)<br>C: 244.89 MiB (243.89–249.58 MiB)<br>S: 254.45 MiB (250.95–266.39 MiB) |
| 800 B | 2,048 条连接 | 0% | 475.75 MiB/s (450.53–536.35 MiB/s)<br>C: 21.03 MiB (15.96–21.16 MiB)<br>S: 6.52 MiB (6.32–6.59 MiB) | 322.52 MiB/s (260.15–386.99 MiB/s)<br>C: 85.75 MiB (85.56–86.81 MiB)<br>S: 71.90 MiB (69.68–73.27 MiB) | 298.04 MiB/s (278.00–312.94 MiB/s)<br>C: 89.44 MiB (87.82–90.04 MiB)<br>S: 73.47 MiB (71.82–75.50 MiB) | 309.85 MiB/s (277.92–354.99 MiB/s)<br>C: 87.36 MiB (86.17–89.85 MiB)<br>S: 70.89 MiB (68.00–73.43 MiB) | 329.25 MiB/s (248.12–373.05 MiB/s)<br>C: 89.25 MiB (88.55–89.43 MiB)<br>S: 71.56 MiB (71.06–76.66 MiB) | 249.87 MiB/s (198.62–290.60 MiB/s)<br>C: 38.88 MiB (38.75–38.88 MiB)<br>S: 38.88 MiB (38.88–39.13 MiB) | 103.79 MiB/s (81.86–112.80 MiB/s)<br>C: 473.59 MiB (472.48–589.80 MiB)<br>S: 497.85 MiB (443.79–550.16 MiB) |
| 800 B | 64 条连接 | 1% | 413.68 MiB/s (352.94–457.15 MiB/s)<br>C: 4.04 MiB (3.96–4.18 MiB)<br>S: 3.27 MiB (3.22–3.46 MiB) | 457.08 MiB/s (388.06–521.29 MiB/s)<br>C: 7.55 MiB (7.32–7.55 MiB)<br>S: 8.13 MiB (7.62–8.40 MiB) | 506.71 MiB/s (433.95–523.45 MiB/s)<br>C: 7.79 MiB (7.62–7.88 MiB)<br>S: 8.28 MiB (7.96–8.47 MiB) | 500.69 MiB/s (385.72–525.45 MiB/s)<br>C: 7.26 MiB (7.12–7.65 MiB)<br>S: 7.98 MiB (7.58–9.02 MiB) | 522.91 MiB/s (503.63–558.91 MiB/s)<br>C: 7.60 MiB (7.32–7.78 MiB)<br>S: 7.93 MiB (7.64–8.17 MiB) | 329.76 MiB/s (291.94–376.96 MiB/s)<br>C: 3.00 MiB (2.88–3.01 MiB)<br>S: 3.25 MiB (3.12–3.25 MiB) | 176.69 MiB/s (147.34–208.96 MiB/s)<br>C: 29.22 MiB (28.43–29.27 MiB)<br>S: 34.60 MiB (31.64–43.68 MiB) |
| 800 B | 256 条连接 | 1% | 483.00 MiB/s (436.33–505.26 MiB/s)<br>C: 5.80 MiB (5.31–5.81 MiB)<br>S: 3.59 MiB (3.50–3.82 MiB) | 506.58 MiB/s (493.75–511.85 MiB/s)<br>C: 15.97 MiB (15.94–16.43 MiB)<br>S: 15.40 MiB (15.38–15.59 MiB) | 483.16 MiB/s (479.45–497.38 MiB/s)<br>C: 17.00 MiB (16.40–17.00 MiB)<br>S: 15.44 MiB (15.05–15.67 MiB) | 499.84 MiB/s (480.30–521.71 MiB/s)<br>C: 16.50 MiB (15.82–16.70 MiB)<br>S: 15.19 MiB (14.90–15.30 MiB) | 483.76 MiB/s (475.20–499.84 MiB/s)<br>C: 16.84 MiB (16.39–17.03 MiB)<br>S: 15.81 MiB (15.68–15.90 MiB) | 410.95 MiB/s (409.96–411.24 MiB/s)<br>C: 6.38 MiB (6.38–6.38 MiB)<br>S: 6.88 MiB (6.75–6.88 MiB) | 179.86 MiB/s (161.81–182.35 MiB/s)<br>C: 76.25 MiB (74.11–77.55 MiB)<br>S: 69.39 MiB (68.77–89.64 MiB) |
| 800 B | 1,024 条连接 | 1% | 497.67 MiB/s (463.11–503.74 MiB/s)<br>C: 12.43 MiB (10.54–12.52 MiB)<br>S: 4.92 MiB (4.81–4.95 MiB) | 454.63 MiB/s (425.51–459.25 MiB/s)<br>C: 49.32 MiB (49.00–50.56 MiB)<br>S: 42.74 MiB (41.58–43.43 MiB) | 453.92 MiB/s (445.74–456.33 MiB/s)<br>C: 50.82 MiB (50.17–51.26 MiB)<br>S: 43.96 MiB (42.48–43.99 MiB) | 424.62 MiB/s (420.96–460.39 MiB/s)<br>C: 49.23 MiB (49.00–49.65 MiB)<br>S: 42.14 MiB (41.87–44.68 MiB) | 402.12 MiB/s (384.49–461.58 MiB/s)<br>C: 48.52 MiB (48.13–52.28 MiB)<br>S: 43.18 MiB (42.58–43.42 MiB) | 346.53 MiB/s (316.37–367.14 MiB/s)<br>C: 20.38 MiB (20.38–20.38 MiB)<br>S: 19.63 MiB (19.38–20.13 MiB) | 77.76 MiB/s (76.97–83.36 MiB/s)<br>C: 315.55 MiB (314.25–348.80 MiB)<br>S: 235.21 MiB (208.95–239.70 MiB) |
| 800 B | 2,048 条连接 | 1% | 469.70 MiB/s (459.18–493.77 MiB/s)<br>C: 19.86 MiB (17.70–21.01 MiB)<br>S: 6.59 MiB (6.56–6.64 MiB) | 341.97 MiB/s (296.70–359.13 MiB/s)<br>C: 88.65 MiB (88.50–90.46 MiB)<br>S: 74.66 MiB (72.72–76.88 MiB) | 369.99 MiB/s (321.91–375.11 MiB/s)<br>C: 92.95 MiB (92.93–92.96 MiB)<br>S: 77.58 MiB (75.41–78.39 MiB) | 355.38 MiB/s (339.30–367.94 MiB/s)<br>C: 89.71 MiB (89.45–89.98 MiB)<br>S: 75.42 MiB (72.52–76.36 MiB) | 324.93 MiB/s (294.61–367.55 MiB/s)<br>C: 93.81 MiB (93.31–94.14 MiB)<br>S: 78.66 MiB (77.67–80.61 MiB) | 325.96 MiB/s (313.13–328.15 MiB/s)<br>C: 38.75 MiB (38.75–38.88 MiB)<br>S: 37.63 MiB (37.50–37.63 MiB) | 34.58 MiB/s (32.33–40.45 MiB/s)<br>C: 748.54 MiB (737.20–782.60 MiB)<br>S: 414.66 MiB (405.23–427.52 MiB) |
| 64 B | 64 条连接 | 0% | 72.39 MiB/s (67.29–73.56 MiB/s)<br>C: 3.91 MiB (3.61–3.99 MiB)<br>S: 3.27 MiB (3.20–3.35 MiB) | 51.88 MiB/s (50.32–51.93 MiB/s)<br>C: 5.88 MiB (5.63–5.89 MiB)<br>S: 6.00 MiB (5.84–6.02 MiB) | 117.59 MiB/s (104.72–118.28 MiB/s)<br>C: 5.73 MiB (5.72–5.75 MiB)<br>S: 6.25 MiB (6.04–6.44 MiB) | 47.80 MiB/s (41.30–48.46 MiB/s)<br>C: 5.73 MiB (5.73–5.83 MiB)<br>S: 5.97 MiB (5.85–6.14 MiB) | 115.16 MiB/s (110.04–118.86 MiB/s)<br>C: 5.81 MiB (5.81–5.88 MiB)<br>S: 6.05 MiB (6.05–6.20 MiB) | 32.89 MiB/s (30.82–34.91 MiB/s)<br>C: 2.38 MiB (2.26–2.38 MiB)<br>S: 2.50 MiB (2.25–2.50 MiB) | 138.29 MiB/s (130.37–139.94 MiB/s)<br>C: 21.77 MiB (21.42–22.12 MiB)<br>S: 19.87 MiB (19.07–20.01 MiB) |
| 64 B | 1,024 条连接 | 0% | 53.63 MiB/s (49.30–54.59 MiB/s)<br>C: 9.65 MiB (9.07–9.67 MiB)<br>S: 4.11 MiB (3.95–4.16 MiB) | 47.00 MiB/s (44.10–47.15 MiB/s)<br>C: 32.38 MiB (32.21–32.65 MiB)<br>S: 27.48 MiB (26.37–27.51 MiB) | 142.00 MiB/s (138.96–144.54 MiB/s)<br>C: 31.88 MiB (31.82–32.49 MiB)<br>S: 25.93 MiB (25.65–25.95 MiB) | 49.86 MiB/s (46.67–50.46 MiB/s)<br>C: 32.77 MiB (32.59–32.99 MiB)<br>S: 27.43 MiB (27.02–28.04 MiB) | 144.83 MiB/s (144.53–147.80 MiB/s)<br>C: 32.33 MiB (32.11–32.51 MiB)<br>S: 26.24 MiB (26.11–26.29 MiB) | 42.64 MiB/s (41.42–43.80 MiB/s)<br>C: 8.88 MiB (8.75–8.88 MiB)<br>S: 9.38 MiB (9.38–9.50 MiB) | 107.73 MiB/s (106.19–109.10 MiB/s)<br>C: 145.95 MiB (139.64–148.52 MiB)<br>S: 95.89 MiB (94.64–96.89 MiB) |
| 64 B | 64 条连接 | 1% | 41.23 MiB/s (40.04–49.52 MiB/s)<br>C: 3.72 MiB (3.70–3.93 MiB)<br>S: 3.30 MiB (3.27–3.32 MiB) | 46.41 MiB/s (43.05–47.78 MiB/s)<br>C: 6.01 MiB (5.88–6.09 MiB)<br>S: 6.12 MiB (6.00–6.23 MiB) | 48.29 MiB/s (42.92–48.35 MiB/s)<br>C: 5.95 MiB (5.87–6.00 MiB)<br>S: 6.45 MiB (6.21–6.51 MiB) | 45.16 MiB/s (44.55–47.56 MiB/s)<br>C: 5.84 MiB (5.80–5.98 MiB)<br>S: 6.12 MiB (6.00–6.14 MiB) | 48.95 MiB/s (45.18–49.53 MiB/s)<br>C: 6.02 MiB (6.01–6.06 MiB)<br>S: 6.36 MiB (6.24–6.38 MiB) | 33.13 MiB/s (31.62–34.48 MiB/s)<br>C: 2.26 MiB (2.25–2.38 MiB)<br>S: 2.38 MiB (2.25–2.50 MiB) | 64.24 MiB/s (48.30–66.69 MiB/s)<br>C: 21.68 MiB (21.57–21.98 MiB)<br>S: 19.05 MiB (18.89–19.82 MiB) |
| 64 B | 1,024 条连接 | 1% | 45.25 MiB/s (45.23–49.36 MiB/s)<br>C: 9.55 MiB (9.08–9.56 MiB)<br>S: 4.23 MiB (4.04–4.36 MiB) | 42.53 MiB/s (42.16–44.21 MiB/s)<br>C: 34.18 MiB (33.88–35.03 MiB)<br>S: 28.25 MiB (27.84–28.43 MiB) | 47.90 MiB/s (45.21–50.35 MiB/s)<br>C: 35.03 MiB (34.45–36.13 MiB)<br>S: 30.71 MiB (29.51–31.03 MiB) | 42.91 MiB/s (41.62–43.62 MiB/s)<br>C: 34.96 MiB (33.21–35.27 MiB)<br>S: 29.32 MiB (29.29–29.82 MiB) | 49.38 MiB/s (42.25–52.09 MiB/s)<br>C: 36.15 MiB (35.83–36.56 MiB)<br>S: 30.37 MiB (30.26–30.45 MiB) | 39.81 MiB/s (39.51–42.11 MiB/s)<br>C: 8.75 MiB (8.75–8.88 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 79.28 MiB/s (74.21–85.47 MiB/s)<br>C: 143.39 MiB (135.39–154.89 MiB)<br>S: 97.64 MiB (97.27–97.77 MiB) |

并发 rust-raknet 使用默认 1400 B 标称 MTU，quic-go 使用 1372 B UDP 数据报预算并关闭路径 MTU 探测。C KCP 使用 1400 B UDP MTU，物理预算多 28 B；本组 64 B／800 B 消息在三者中均不分片。

### 持续负载下的交付延迟

并发客户端使用 `--loaded-rtt`，发送前打时间戳，在消费已验证回显时计算 RTT，贯穿突发全过程，包含客户端和服务端排队。每轮验证 262144 条突发回显，保持相同的 16 条应用窗口、worker、CPU 绑定和服务端配置，各场景 3 轮。

延迟采样与吞吐表分开运行，不把该组带采样的吞吐混入上方吞吐表。RSS 包括客户端保存、汇总和排序的 RTT 样本。

| 消息大小 | 连接数 | 注入丢包 | TCP<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet (`send`)<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口）<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet (`send` + `send-policy`)<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | rust-raknet（批量接口 + `send-policy`）<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | C KCP<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S | quic-go<br>RTT 中位数 / 99%（范围）<br>峰值 RSS：C / S |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 64 B | 64 条连接 | 0% | 50%: 0.96 ms (0.92–0.97 ms)<br>99%: 4.59 ms (3.79–8.62 ms)<br>C: 11.70 MiB (11.48–11.79 MiB)<br>S: 3.16 MiB (3.10–3.18 MiB) | 50%: 1.11 ms (0.96–1.13 ms)<br>99%: 4.13 ms (3.35–7.12 ms)<br>C: 13.30 MiB (13.23–13.52 MiB)<br>S: 5.79 MiB (5.59–5.94 MiB) | 50%: 0.46 ms (0.41–0.49 ms)<br>99%: 4.29 ms (3.63–5.45 ms)<br>C: 13.21 MiB (13.07–13.47 MiB)<br>S: 5.93 MiB (5.88–6.04 MiB) | 50%: 1.08 ms (1.04–1.28 ms)<br>99%: 4.23 ms (3.86–8.13 ms)<br>C: 13.50 MiB (13.45–13.59 MiB)<br>S: 5.82 MiB (5.66–5.84 MiB) | 50%: 0.46 ms (0.45–0.48 ms)<br>99%: 4.18 ms (1.05–4.20 ms)<br>C: 13.31 MiB (13.29–13.61 MiB)<br>S: 5.85 MiB (5.80–6.04 MiB) | 50%: 1.31 ms (1.30–1.32 ms)<br>99%: 10.16 ms (6.87–11.64 ms)<br>C: 6.26 MiB (6.12–6.36 MiB)<br>S: 2.50 MiB (2.25–2.50 MiB) | 50%: 0.37 ms (0.34–0.41 ms)<br>99%: 1.89 ms (1.84–3.18 ms)<br>C: 32.32 MiB (31.20–33.02 MiB)<br>S: 17.45 MiB (17.25–17.52 MiB) |
| 64 B | 1,024 条连接 | 0% | 50%: 18.99 ms (17.77–19.36 ms)<br>99%: 35.03 ms (32.64–35.43 ms)<br>C: 16.77 MiB (16.26–16.99 MiB)<br>S: 4.04 MiB (3.90–4.18 MiB) | 50%: 16.90 ms (15.81–18.16 ms)<br>99%: 94.65 ms (40.14–95.26 ms)<br>C: 36.48 MiB (36.36–36.76 MiB)<br>S: 23.37 MiB (22.90–24.15 MiB) | 50%: 4.70 ms (4.46–4.85 ms)<br>99%: 19.61 ms (19.57–20.56 ms)<br>C: 37.57 MiB (37.43–38.11 MiB)<br>S: 24.38 MiB (24.24–24.44 MiB) | 50%: 15.81 ms (15.25–17.53 ms)<br>99%: 101.42 ms (35.93–104.92 ms)<br>C: 36.51 MiB (36.48–36.63 MiB)<br>S: 24.37 MiB (23.33–24.41 MiB) | 50%: 4.86 ms (4.83–5.30 ms)<br>99%: 19.19 ms (18.10–21.89 ms)<br>C: 38.03 MiB (37.87–38.07 MiB)<br>S: 24.65 MiB (24.57–24.85 MiB) | 50%: 7.37 ms (6.73–8.61 ms)<br>99%: 120.25 ms (112.92–160.63 ms)<br>C: 10.69 MiB (10.69–10.94 MiB)<br>S: 9.38 MiB (9.38–9.38 MiB) | 50%: 9.01 ms (8.97–9.71 ms)<br>99%: 13.32 ms (12.90–16.47 ms)<br>C: 140.02 MiB (137.39–140.58 MiB)<br>S: 88.89 MiB (87.69–90.89 MiB) |
| 64 B | 64 条连接 | 1% | 50%: 0.62 ms (0.61–0.72 ms)<br>99%: 5.88 ms (5.06–6.54 ms)<br>C: 11.54 MiB (11.50–11.73 MiB)<br>S: 3.28 MiB (3.22–3.29 MiB) | 50%: 0.92 ms (0.88–1.02 ms)<br>99%: 4.97 ms (4.60–6.16 ms)<br>C: 13.48 MiB (13.46–13.79 MiB)<br>S: 5.91 MiB (5.89–6.00 MiB) | 50%: 0.92 ms (0.87–0.97 ms)<br>99%: 4.54 ms (3.68–4.82 ms)<br>C: 13.62 MiB (13.59–13.90 MiB)<br>S: 6.17 MiB (6.09–6.35 MiB) | 50%: 0.95 ms (0.91–0.96 ms)<br>99%: 6.20 ms (3.96–6.51 ms)<br>C: 13.74 MiB (13.62–13.77 MiB)<br>S: 5.92 MiB (5.86–6.22 MiB) | 50%: 0.96 ms (0.91–1.00 ms)<br>99%: 4.82 ms (4.29–4.84 ms)<br>C: 13.81 MiB (13.71–14.07 MiB)<br>S: 6.24 MiB (6.21–6.24 MiB) | 50%: 1.25 ms (1.21–1.44 ms)<br>99%: 7.37 ms (7.33–8.08 ms)<br>C: 6.34 MiB (6.15–6.36 MiB)<br>S: 2.38 MiB (2.38–2.38 MiB) | 50%: 0.10 ms (0.10–0.12 ms)<br>99%: 27.24 ms (27.23–27.36 ms)<br>C: 32.52 MiB (31.79–34.39 MiB)<br>S: 17.32 MiB (16.83–17.37 MiB) |
| 64 B | 1,024 条连接 | 1% | 50%: 16.30 ms (16.20–17.18 ms)<br>99%: 40.97 ms (38.60–43.32 ms)<br>C: 16.58 MiB (15.57–17.49 MiB)<br>S: 3.91 MiB (3.89–3.95 MiB) | 50%: 18.21 ms (17.97–18.25 ms)<br>99%: 61.24 ms (57.43–66.63 ms)<br>C: 37.43 MiB (37.01–37.52 MiB)<br>S: 24.75 MiB (24.47–24.75 MiB) | 50%: 12.36 ms (12.18–15.99 ms)<br>99%: 39.28 ms (36.48–96.98 ms)<br>C: 38.88 MiB (37.86–39.56 MiB)<br>S: 25.61 MiB (25.61–26.52 MiB) | 50%: 16.61 ms (16.42–18.79 ms)<br>99%: 109.33 ms (65.97–109.70 ms)<br>C: 36.77 MiB (35.29–39.10 MiB)<br>S: 26.01 MiB (25.34–26.05 MiB) | 50%: 12.45 ms (11.62–15.14 ms)<br>99%: 43.40 ms (35.63–84.78 ms)<br>C: 37.48 MiB (36.12–38.46 MiB)<br>S: 26.17 MiB (25.97–26.54 MiB) | 50%: 5.90 ms (5.03–8.88 ms)<br>99%: 161.43 ms (111.57–165.05 ms)<br>C: 10.94 MiB (10.82–10.94 MiB)<br>S: 9.25 MiB (9.25–9.38 MiB) | 50%: 8.92 ms (8.38–10.94 ms)<br>99%: 59.60 ms (47.84–73.50 ms)<br>C: 134.08 MiB (131.52–138.64 MiB)<br>S: 89.07 MiB (86.48–90.95 MiB) |
| 800 B | 64 条连接 | 0% | 50%: 1.18 ms (1.16–1.22 ms)<br>99%: 8.77 ms (5.87–9.36 ms)<br>C: 11.88 MiB (11.76–11.89 MiB)<br>S: 3.37 MiB (3.14–3.42 MiB) | 50%: 1.15 ms (1.15–1.20 ms)<br>99%: 8.05 ms (3.26–8.41 ms)<br>C: 14.66 MiB (14.49–14.85 MiB)<br>S: 7.12 MiB (6.91–7.78 MiB) | 50%: 1.16 ms (1.14–1.22 ms)<br>99%: 8.84 ms (8.39–9.00 ms)<br>C: 14.82 MiB (14.81–14.83 MiB)<br>S: 7.30 MiB (7.29–7.34 MiB) | 50%: 1.13 ms (1.10–1.16 ms)<br>99%: 9.05 ms (9.02–9.21 ms)<br>C: 14.71 MiB (14.60–14.85 MiB)<br>S: 7.17 MiB (7.17–7.19 MiB) | 50%: 1.21 ms (1.18–1.27 ms)<br>99%: 5.42 ms (3.84–8.76 ms)<br>C: 15.00 MiB (14.90–15.01 MiB)<br>S: 7.07 MiB (7.04–7.62 MiB) | 50%: 1.17 ms (1.16–1.21 ms)<br>99%: 16.37 ms (15.58–17.62 ms)<br>C: 6.96 MiB (6.93–6.97 MiB)<br>S: 3.13 MiB (3.13–3.25 MiB) | 50%: 3.08 ms (3.03–3.12 ms)<br>99%: 11.50 ms (9.64–12.21 ms)<br>C: 37.70 MiB (37.45–40.16 MiB)<br>S: 30.24 MiB (29.33–31.19 MiB) |
| 800 B | 1,024 条连接 | 0% | 50%: 22.70 ms (21.95–24.31 ms)<br>99%: 42.70 ms (41.55–75.24 ms)<br>C: 19.46 MiB (18.15–20.46 MiB)<br>S: 5.02 MiB (4.86–5.08 MiB) | 50%: 21.35 ms (13.79–21.76 ms)<br>99%: 114.12 ms (58.20–120.21 ms)<br>C: 50.53 MiB (50.32–51.90 MiB)<br>S: 35.95 MiB (34.48–36.38 MiB) | 50%: 16.88 ms (10.23–18.16 ms)<br>99%: 117.16 ms (107.80–190.99 ms)<br>C: 50.95 MiB (49.53–51.82 MiB)<br>S: 35.30 MiB (34.23–35.42 MiB) | 50%: 20.42 ms (16.57–21.18 ms)<br>99%: 42.81 ms (40.26–117.52 ms)<br>C: 50.59 MiB (50.52–50.68 MiB)<br>S: 36.27 MiB (34.35–36.39 MiB) | 50%: 12.39 ms (10.59–18.19 ms)<br>99%: 169.10 ms (109.53–187.16 ms)<br>C: 51.48 MiB (51.21–51.66 MiB)<br>S: 35.09 MiB (34.91–36.32 MiB) | 50%: 18.82 ms (18.65–20.18 ms)<br>99%: 107.54 ms (102.72–118.36 ms)<br>C: 22.19 MiB (22.07–22.32 MiB)<br>S: 20.63 MiB (20.12–20.75 MiB) | 50%: 71.29 ms (69.26–71.72 ms)<br>99%: 140.18 ms (109.03–283.41 ms)<br>C: 244.89 MiB (222.95–249.27 MiB)<br>S: 150.57 MiB (149.33–175.30 MiB) |
| 800 B | 64 条连接 | 1% | 50%: 0.64 ms (0.63–0.66 ms)<br>99%: 7.02 ms (6.99–7.43 ms)<br>C: 11.82 MiB (11.67–11.84 MiB)<br>S: 3.30 MiB (3.23–3.39 MiB) | 50%: 1.03 ms (1.02–1.19 ms)<br>99%: 5.91 ms (5.70–6.11 ms)<br>C: 15.04 MiB (14.72–15.32 MiB)<br>S: 7.23 MiB (7.18–7.94 MiB) | 50%: 1.05 ms (1.03–1.10 ms)<br>99%: 6.06 ms (5.42–6.52 ms)<br>C: 15.16 MiB (14.84–15.44 MiB)<br>S: 7.55 MiB (7.46–7.62 MiB) | 50%: 1.08 ms (0.95–1.16 ms)<br>99%: 5.85 ms (5.10–6.30 ms)<br>C: 14.98 MiB (14.89–15.22 MiB)<br>S: 7.32 MiB (7.31–7.63 MiB) | 50%: 1.02 ms (1.02–1.04 ms)<br>99%: 5.90 ms (5.61–6.11 ms)<br>C: 14.98 MiB (14.95–15.06 MiB)<br>S: 7.50 MiB (7.50–7.53 MiB) | 50%: 1.24 ms (1.03–1.31 ms)<br>99%: 11.33 ms (6.75–11.36 ms)<br>C: 6.79 MiB (6.73–6.89 MiB)<br>S: 3.13 MiB (3.00–3.25 MiB) | 50%: 3.20 ms (3.15–3.29 ms)<br>99%: 13.52 ms (12.98–23.95 ms)<br>C: 40.48 MiB (36.59–41.42 MiB)<br>S: 29.52 MiB (28.62–33.45 MiB) |
| 800 B | 1,024 条连接 | 1% | 50%: 25.00 ms (19.25–26.59 ms)<br>99%: 82.13 ms (53.30–85.70 ms)<br>C: 20.33 MiB (20.15–20.54 MiB)<br>S: 4.97 MiB (4.71–5.04 MiB) | 50%: 21.11 ms (19.79–22.75 ms)<br>99%: 122.90 ms (90.12–136.39 ms)<br>C: 51.90 MiB (50.38–52.18 MiB)<br>S: 36.71 MiB (36.62–37.41 MiB) | 50%: 30.06 ms (22.12–30.13 ms)<br>99%: 340.38 ms (193.22–355.15 ms)<br>C: 53.13 MiB (52.59–54.25 MiB)<br>S: 38.64 MiB (37.71–39.57 MiB) | 50%: 19.21 ms (17.43–30.82 ms)<br>99%: 195.16 ms (105.69–200.33 ms)<br>C: 51.19 MiB (49.07–53.82 MiB)<br>S: 36.22 MiB (35.62–37.24 MiB) | 50%: 18.11 ms (13.85–21.74 ms)<br>99%: 212.59 ms (201.87–336.37 ms)<br>C: 53.09 MiB (52.64–53.35 MiB)<br>S: 37.80 MiB (37.38–37.97 MiB) | 50%: 23.50 ms (13.31–24.95 ms)<br>99%: 175.31 ms (136.41–264.19 ms)<br>C: 22.32 MiB (22.07–22.32 MiB)<br>S: 19.88 MiB (18.75–20.00 MiB) | 50%: 78.05 ms (75.61–81.64 ms)<br>99%: 365.24 ms (338.86–430.47 ms)<br>C: 245.64 MiB (240.02–252.82 MiB)<br>S: 191.64 MiB (190.14–205.88 MiB) |

### 如何理解这些结果

- 64 B、1,024 条连接、无注入丢包时，默认普通发送为 **47.00 MiB/s**，批量接口为 **142.00 MiB/s**；TCP、C KCP、quic-go 分别为 **53.63 MiB/s、42.64 MiB/s、107.73 MiB/s**。
- 单连接、800 B、5% 丢包时，默认普通发送为 **146.72 MiB/s**；TCP、C KCP、quic-go 分别为 **1.03 MiB/s、102.06 MiB/s、8.07 MiB/s**。这些结果对应配置明确的可靠有序回显负载。
- 800 B、1,024 条连接、无注入丢包时，配置策略让普通吞吐从 **372.54 MiB/s 变为 395.28 MiB/s**，批量吞吐从 **413.36 MiB/s 变为 381.62 MiB/s**。
- 64 B、1,024 条连接、无注入丢包时，配置策略让普通持续负载 RTT 中位数从 **16.90 ms 变为 15.81 ms**，99% RTT 从 **94.65 ms 变为 101.42 ms**。吞吐、慢消息延迟和内存应一起比较。

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
python3 scripts/test-features.py
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
