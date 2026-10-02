# Network benchmark

Run these commands only in a disposable container or task copy with a private network namespace and task-local Cargo caches. Run both echo servers in separate terminals, then run each client with identical options. TCP and UDP can bind the same numeric port on one host. Use release mode for comparable performance numbers.

Start the TCP echo server in one terminal:

```sh
cargo run --release --manifest-path examples/test_benchmark/Cargo.toml -- \
  --protocol tcp --type server --address 127.0.0.1:19132
```

Start the `rust-raknet` echo server in another terminal:

```sh
cargo run --release --manifest-path examples/test_benchmark/Cargo.toml -- \
  --protocol raknet --type server --address 127.0.0.1:19132
```

Run both clients with matching options, one at a time:

```sh
cargo run --release --manifest-path examples/test_benchmark/Cargo.toml -- \
  --protocol tcp --type client --address 127.0.0.1:19132 \
  --packets 20000 --payload-size 800 --warmup 200 --latency-samples 300

cargo run --release --manifest-path examples/test_benchmark/Cargo.toml -- \
  --protocol raknet --type client --address 127.0.0.1:19132 \
  --packets 20000 --payload-size 800 --warmup 200 --latency-samples 300
```

Client defaults are 10,000 measured packets, 800-byte payloads, 100 warmup rounds, and 100 RTT samples. Set `--packets`, `--payload-size`, `--warmup`, or `--latency-samples` to adjust them; `--latency-samples 0` skips latency measurement. For a fair direct comparison, keep all client options identical and alternate the protocol run order if repeating measurements.

The benchmark runs its main task inside the Tokio worker pool for both protocols. The client warms up with sequential request/echo rounds, measures full request/echo RTT percentiles, then sends the measured packet burst while receiving echoes concurrently. Both protocols use a sliding application window of 64 messages, releasing one slot only after a verified echo. `rust-raknet` uses `ReliableOrdered`; TCP uses length-prefixed records and `TCP_NODELAY`, writes each whole record in one call, and reuses its server record buffer. Reported MiB/s counts application payload in one direction, excluding protocol headers and acknowledgements. RTT includes both client and server processing and the local network path.

Pass `--raknet-mtu 1428` to both the `rust-raknet` server and client for the README's
single-connection C KCP comparison. Its default is 1,400 bytes. C KCP's 1,400-byte
UDP MTU excludes the 28-byte IPv4/UDP headers, so a nominal `rust-raknet` MTU of 1,428
bytes gives both protocols the same IPv4 packet budget. Keep default-MTU
regression checks separate from this explicitly configured comparison.

## Packet-loss comparison

On Linux, run `examples/test_benchmark/run_loss_comparison.sh` to compare 0%, 1%, and 5% packet loss, with three runs per protocol and profile. It starts both echo servers and clients inside a new user/network namespace, applies `tc netem` only to that namespace's loopback, limits loopback MTU to 1,500 bytes, and limits GSO/GRO to one packet. It prints `tc -s qdisc` counters after each run so the injected loss can be checked. If namespace creation is unavailable, the script fails before adding a loss rule; it has no host-network fallback.

The loss runner requires Linux `bwrap`, `ip`, `tc`, and `timeout`. It never builds on the host and accepts no worker/bypass arguments. Build the binary inside your isolated development environment first, then point the runner at that executable:

```sh
RAKNET_BENCHMARK=/path/to/task/target/release/test_benchmark \
  bash examples/test_benchmark/run_loss_comparison.sh
```

The runner copies that binary into a disposable directory, hides the host home directory, mounts the system read-only, and checks that the network namespace differs from its parent before changing the loopback. Each client run is bounded to 180 seconds. It requires no root privileges on the host. If the required namespace capabilities are unavailable, use a disposable VM instead; do not apply loss rules to a host interface.

## Comparing revisions

Build this benchmark against the baseline and candidate in separate disposable copies using identical Rust versions and resolved dependencies. Then run:

```sh
bash examples/test_benchmark/compare_revisions.sh \
  /path/to/baseline/test_benchmark /path/to/candidate/test_benchmark \
  > comparison.jsonl
```

This runner creates its own isolated namespace. It alternates baseline/candidate order, runs three repetitions, and repeats the TCP control equally for each profile: 800-byte clean/1%/5% loss, 64-byte small packets, 4 KiB fragmented messages, and 800-byte messages with 1% loss plus 5 ms delay per direction. Each result contains RTT percentiles, throughput, client/server CPU and peak RSS, raw benchmark output, and qdisc counters. Failures remain in the JSONL and cause a nonzero final exit status. Latency percentiles are sensitive to random loss; increase repetitions before treating a small difference as a regression. This measures one connection per process; it does not characterize high connection counts or Internet congestion fairness.

For larger latency samples, add `--latency` (five pairs, 10,000 RTT samples each, no loss) or `--wan-latency` (three pairs, 2,000 samples each, 1% loss and 5 ms each way). These modes pin the client and server to opposite ends of the available CPU affinity list and require `taskset` and at least two available CPUs. TCP uses the same repetition count as `rust-raknet` in each mode. Add `--latency-unpinned` for the same clean latency sample count without CPU pinning; keep pinned and unpinned results separate. Add `--throughput-long` to extend the throughput bursts to 200,000 / 300,000 / 50,000 messages at 800 / 64 / 4,096 bytes respectively. That mode uses 200,000 messages for both the 1% and 5% loss bursts, plus 3,000 messages for the delay profile. Each comparison client has a 300-second limit, including long TCP loss bursts. Use a separate output file for each mode and set `TOKIO_WORKER_THREADS=4` for the worker count used in the latest validation.

## Concurrent proxy validation

Build `examples/concurrency.rs`, the echo benchmark server, and `examples/proxy` in task copies with private caches. Start the server and proxy inside the same disposable network namespace, then run:

```sh
/path/to/task/target/release/examples/concurrency 127.0.0.1:19201 1024 30
```

The driver establishes connections with eight concurrent handshakes, keeps all connections open before sending, and validates 64 / 800 / 4,096-byte echoes with unique connection/message IDs. The receiving application pauses for 30 ms to exercise buffering. This uses a transport echo workload. The timeout is 120 seconds. Apply any loss rules only to the disposable namespace and inspect its UDP receive-buffer errors as well as netem counters.

## Concurrent throughput benchmark

Build `examples/concurrency_benchmark.rs` and `examples/sharded_echo.rs` in the
task copy. Run all processes inside the same private network namespace. Run one
protocol at a time, starting its server before its client. For 1,024 connections:

```sh
# `rust-raknet` server and client, in separate terminals inside the namespace.
TOKIO_WORKER_THREADS=4 taskset -c 0-3 /path/to/task/target/release/examples/sharded_echo 127.0.0.1:19132 4
taskset -c 4-7 /path/to/task/target/release/examples/concurrency_benchmark 127.0.0.1:19132 1024 1024 800

# TCP server and client, in separate terminals inside the namespace.
taskset -c 0-3 /path/to/task/target/release/examples/concurrency_benchmark --tcp-server 127.0.0.1:19132
taskset -c 4-7 /path/to/task/target/release/examples/concurrency_benchmark --tcp 127.0.0.1:19132 1024 1024 800
```

Choose CPU lists from the CPUs actually available to the sandbox. Both Rust
clients use the same warmup, barriers, echo validation and sliding window. TCP
uses four Linux `SO_REUSEPORT` listeners, four Tokio workers, `TCP_NODELAY`,
four-byte little-endian length prefixes, and one complete-record write. Both TCP
ends reuse record buffers. TCP may combine several records in a segment; loss
percentages apply to network packets, not application messages. The TCP server
mode requires Linux.

The arguments are address, connection count, messages per connection, and payload
size in bytes (at least 17). The driver uses eight concurrent handshakes and four
Tokio workers. Every connection first measures 20 sequential round trips. A
barrier then starts the throughput burst, with up to 16 messages in flight per
connection, refilling a slot after each echo rather than waiting for a whole batch.
Every ordered echo is checked against its connection and message ID.
Connections remain open until all bursts finish. The timeout is 180 seconds.

Without `--loaded-rtt`, setup and RTT sampling precede the throughput timer.
Those RTT percentiles describe the sequential phase. Append `--loaded-rtt` to
the client command for per-message RTT throughout the measured burst; run that
instrumented workload separately from throughput. These measurements use transport echo workloads.
Inspect namespace-local UDP error counters; no injected loss does not guarantee
that socket buffers never overflow.

On Linux, `examples/sharded_echo.rs` accepts an address and optional receive-socket
count (default four). Compare against an ordinary listener using identical client
settings. Socket sharding is workload-dependent; the proxy defaults to one socket.

## Official C KCP comparison

The [C adapters](kcp/README.md) use upstream `ikcp.c` directly. Build them in the
same isolated task as the Rust binaries. Use identical payloads and counts,
separate client/server CPU sets, and alternating protocol order. Both drivers
reuse their payload templates and verify the complete echoed payload.

For sustained concurrent throughput, the README uses 1,048,576 messages per run:
64 / 256 / 1,024 / 2,048 connections send 16,384 / 4,096 / 1,024 / 512 messages
each. Use 800-byte payloads, a sliding window of 16 per connection, four workers
and four server receive sockets. Repeat each 0% / 1% loss case three times and
report median and range. These use reliable ordered echo workloads. Include TCP using the concurrent `--tcp-server` / `--tcp` modes above;
do not substitute the single-connection driver. Rotate TCP / `rust-raknet` / C KCP run
order across repetitions. Apply `tc netem` only after confirming a private network
namespace, with loopback MTU 1,500 B, GSO/GRO limited to one packet and queue limit
100,000 packets. Record qdisc and UDP error counters for every run. Affinity does
not reserve CPUs exclusively; report each protocol's median and full range.

## Explicit message batching

For a separate measurement of the batch API, pass `--raknet-batch-size 16` to
both the RakNet server and client. The default is 1 and keeps individual sends.
The client takes only currently available application-window slots: it never
waits to fill a batch. The echo server drains only messages already available.
The window stays at 64 messages, and every echoed payload is checked.

For concurrent runs, use `sharded_echo ADDRESS 4 --batch-messages` and
`concurrency_benchmark --batch ADDRESS CONNECTIONS MESSAGES PAYLOAD`.
The application window stays at 16 messages per connection. The README reports two columns from the same library build:

- **rust-raknet (`send`)**: neither endpoint calls a batch API. Use
  `--raknet-batch-size 1` on the single-connection server and client, and omit
  `--batch-messages` / `--batch` for concurrent runs.
- **rust-raknet (batch APIs)**: use `--raknet-batch-size 16` on both single-connection
  endpoints, or the concurrent flags above. The client uses `send_batch` for the
  single-connection burst and `send_bytes_batch` for concurrent bursts; the server
  uses `recv_bytes_batch` and `send_bytes_batch`.

Start fresh servers and connections for each run. Batch mode is connection-wide:
ordinary sends can participate after a batch call has enabled packing. Keeping
both endpoints in one mode avoids mixing that state into the ordinary results.

The configured API batch size is not the number of messages per UDP datagram.
Packing is limited to eight non-fragmented `ReliableOrdered` messages and the
negotiated MTU. At the tested MTUs, two 800 B messages do not fit together;
4,096 B messages are fragmented and are not packed. Batch API results for these
payloads therefore measure the API and receive-draining path, not packet merging.
Any effective NACK or reliable retransmission timeout disables packing for the
rest of that connection, including loss observed during warmup. Batch API calls
continue to work after fallback, but send individual datagrams.

Sequential latency samples in batch mode call `send_batch` with one message;
there is no second message to merge. They measure sparse request/echo latency,
not sustained-load latency. Without `--loaded-rtt`, the concurrent driver's RTT output comes from the
20 pre-burst samples. All three concurrent client drivers support `--loaded-rtt`
for separate sustained-load latency measurements.

Rotate TCP / ordinary `rust-raknet` / batch `rust-raknet` / C KCP / quic-go order
across repetitions. Use the same application windows, message counts, network
profiles and CPU placement for every implementation. Record namespace-local UDP
errors and qdisc statistics with each result, including runs without injected loss.


## Reading the current comparison

The README's October 2, 2026 tables use three throughput repetitions per profile
and five sparse-latency repetitions (1,000 warmups and 10,000 samples each).
Single-connection windows stay at 64 messages; concurrent windows stay at 16.
Concurrent bursts verify 1,048,576 unique ordered echoes at 64 / 256 / 1,024 /
2,048 connections for 800 B, and 64 / 1,024 connections for 64 B. These run with
0% / 1% injected loss. The single-connection tables also include 5% loss and
1% loss plus 5 ms delay each way; message counts are shown with each row.

The concurrent batch client constructs owned payloads for pending messages,
while the ordinary client reuses a borrowed template. Results include these
buffer choices and receive draining; they are not a pure packet-packing ablation.
The Rust, C KCP and quic-go concurrent clients support `--loaded-rtt`. They
timestamp messages before sending, verifies 262,144 burst echoes, and measures RTT as each echo is consumed
throughout the burst. It retains the same 16-message window and CPU placement,
and runs three repetitions at 64 / 1,024 connections for 64 B / 800 B and
0% / 1% loss. Its instrumentation cost is excluded from throughput tables.

## Full five-column comparison and peak memory

The [quic-go drivers](quic/README.md) are included here alongside the C KCP
adapters. `run_comparison.py` runs TCP, ordinary rust-raknet, batch rust-raknet,
C KCP and quic-go with rotating order and fresh servers for every repetition.
It fails if the network namespace is not private, Cargo/HOME are not task-local,
message/sample counts are incorrect, or a process memory measurement is missing.
No host-network fallback is provided.

Prepare this layout **inside an isolated task directory**, with all toolchains
and caches belonging to that task:

```text
TASK_ROOT/
  work/                  repository copy
  home/ cargo/           task HOME and CARGO_HOME
  go/                    task-local Go toolchain
  target/release/         Rust binaries and examples
  bin/                   C and Go benchmark executables
  logs/ results/         raw output (keep outside the repository)
  kcp/                   pinned upstream ikcp.c, ikcp.h and license
```

Build the Rust benchmark and examples in that task:

```sh
cargo build --offline --release --manifest-path examples/test_benchmark/Cargo.toml
cargo build --offline --release --example concurrency_benchmark --example sharded_echo
cc -O2 -std=c11 examples/test_benchmark/measure_process.c -o "$TASK_ROOT/bin/measure-process"
```

Build the C adapters as `bin/kcp-single` and `bin/kcp-concurrent`, and the Go
drivers as `bin/quic-single` and `bin/quic-concurrent`. Set `CARGO_TARGET_DIR`
to `TASK_ROOT/target`. The runner expects prebuilt binaries and never downloads
dependencies or builds during measurement. Linux `ip`, `tc`, `taskset`, `nice`,
`lscpu`, Python 3 and at least eight available CPUs are required.

Run from `TASK_ROOT/work` inside a private user/network namespace with UID mapping
of one user to namespace UID 0 and namespace-local `CAP_NET_ADMIN`. Pass the
parent's network namespace identifier as `TASK_HOST_NETNS` **before entering**
the namespace. Set task-local `HOME`, `CARGO_HOME`, Rust/Go caches and temporary
directories. Mount the host read-only and hide its home directory. Then run:

```sh
python3 examples/test_benchmark/run_comparison.py single > "$TASK_ROOT/results/single.jsonl"
python3 examples/test_benchmark/run_comparison.py concurrent > "$TASK_ROOT/results/concurrent.jsonl"
LOADED_RTT=1 python3 examples/test_benchmark/run_comparison.py concurrent > "$TASK_ROOT/results/loaded.jsonl"
python3 examples/test_benchmark/run_comparison.py latency > "$TASK_ROOT/results/latency.jsonl"
```

The runner uses 3 repetitions for every single/concurrent/loaded profile, and 5
for sparse latency. `SMOKE=1` selects short one-repetition checks instead. Raw
JSONL contains each process's `wait4` resource usage, verified echo/sample counts,
network counters and output. A failure is retained and stops the suite.

The small C supervisor forwards termination signals to the server and records
Linux `ru_maxrss` after the child exits. There is no RSS polling during the run.
Divide KiB by 1,024 to report MiB. Values are **whole-process peak resident
memory**, covering startup, warmup, burst, reporting and teardown, including
runtime/allocator/application buffers. They exclude kernel socket buffers and
are neither heap-only measurements nor memory per connection. TCP transport
state lives in the kernel, so RSS is not total system memory used for networking. Client/server
peaks can occur at different times; report them separately. The loaded clients
also retain all RTT samples, so their memory belongs in a separate table.

For each profile, report the median of three process peaks alongside throughput.
RSS ranges and throughput ranges are useful when comparing close results.
