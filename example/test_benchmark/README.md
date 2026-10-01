# Network benchmark

Run these commands only in a disposable container or task copy with a private network namespace and task-local Cargo caches. Run both echo servers in separate terminals, then run each client with identical options. TCP and UDP can bind the same numeric port on one host. Use release mode for comparable performance numbers.

Start the TCP echo server in one terminal:

```sh
cargo run --release --manifest-path example/test_benchmark/Cargo.toml -- \
  --protocol tcp --type server --address 127.0.0.1:19132
```

Start the RakNet echo server in another terminal:

```sh
cargo run --release --manifest-path example/test_benchmark/Cargo.toml -- \
  --protocol raknet --type server --address 127.0.0.1:19132
```

Run both clients with matching options, one at a time:

```sh
cargo run --release --manifest-path example/test_benchmark/Cargo.toml -- \
  --protocol tcp --type client --address 127.0.0.1:19132 \
  --packets 20000 --payload-size 800 --warmup 200 --latency-samples 300

cargo run --release --manifest-path example/test_benchmark/Cargo.toml -- \
  --protocol raknet --type client --address 127.0.0.1:19132 \
  --packets 20000 --payload-size 800 --warmup 200 --latency-samples 300
```

Client defaults are 10,000 measured packets, 800-byte payloads, 100 warmup rounds, and 100 RTT samples. Set `--packets`, `--payload-size`, `--warmup`, or `--latency-samples` to adjust them; `--latency-samples 0` skips latency measurement. For a fair direct comparison, keep all client options identical and alternate the protocol run order if repeating measurements.

The benchmark runs its main task inside the Tokio worker pool for both protocols. The client warms up with sequential request/echo rounds, measures full request/echo RTT percentiles, then sends the measured packet burst while receiving echoes concurrently. Both protocols use a sliding application window of 64 messages, releasing one slot only after a verified echo. RakNet uses `ReliableOrdered`; TCP uses length-prefixed records and `TCP_NODELAY`, writes each whole record in one call, and reuses its server record buffer. Reported MiB/s counts application payload in one direction, excluding protocol headers and acknowledgements. RTT includes both client and server processing and the local network path.

Pass `--raknet-mtu 1428` to both the RakNet server and client for the README's
single-connection C KCP comparison. Its default is 1,400 bytes. C KCP's 1,400-byte
UDP MTU excludes the 28-byte IPv4/UDP headers, so a nominal RakNet MTU of 1,428
bytes gives both protocols the same IPv4 packet budget. Keep default-MTU
regression checks separate from this explicitly configured comparison.

## Packet-loss comparison

On Linux, run `example/test_benchmark/run_loss_comparison.sh` to compare 0%, 1%, and 5% packet loss, with three runs per protocol and profile. It starts both echo servers and clients inside a new user/network namespace, applies `tc netem` only to that namespace's loopback, limits loopback MTU to 1,500 bytes, and limits GSO/GRO to one packet. It prints `tc -s qdisc` counters after each run so the injected loss can be checked. If namespace creation is unavailable, the script fails before adding a loss rule; it has no host-network fallback.

The loss runner requires Linux `bwrap`, `ip`, `tc`, and `timeout`. It never builds on the host and accepts no worker/bypass arguments. Build the binary inside your isolated development environment first, then point the runner at that executable:

```sh
RAKNET_BENCHMARK=/path/to/task/target/release/test_benchmark \
  bash example/test_benchmark/run_loss_comparison.sh
```

The runner copies that binary into a disposable directory, hides the host home directory, mounts the system read-only, and checks that the network namespace differs from its parent before changing the loopback. Each client run is bounded to 180 seconds. It requires no root privileges on the host. If the required namespace capabilities are unavailable, use a disposable VM instead; do not apply loss rules to a host interface.

## Comparing revisions

Build this benchmark against the baseline and candidate in separate disposable copies using identical Rust versions and resolved dependencies. Then run:

```sh
bash example/test_benchmark/compare_revisions.sh \
  /path/to/baseline/test_benchmark /path/to/candidate/test_benchmark \
  > comparison.jsonl
```

This runner creates its own isolated namespace. It alternates baseline/candidate order, runs three repetitions, and repeats the TCP control equally for each profile: 800-byte clean/1%/5% loss, 64-byte small packets, 4 KiB fragmented messages, and 800-byte messages with 1% loss plus 5 ms delay per direction. Each result contains RTT percentiles, throughput, client/server CPU and peak RSS, raw benchmark output, and qdisc counters. Failures remain in the JSONL and cause a nonzero final exit status. Latency percentiles are sensitive to random loss; increase repetitions before treating a small difference as a regression. This measures one connection per process; it does not characterize high connection counts or Internet congestion fairness.

For larger latency samples, add `--latency` (five pairs, 10,000 RTT samples each, no loss) or `--wan-latency` (three pairs, 2,000 samples each, 1% loss and 5 ms each way). These modes pin the client and server to opposite ends of the available CPU affinity list and require `taskset` and at least two available CPUs. TCP uses the same repetition count as RakNet in each mode. Add `--latency-unpinned` for the same clean latency sample count without CPU pinning; keep pinned and unpinned results separate. Add `--throughput-long` to extend the throughput bursts to 200,000 / 300,000 / 50,000 messages at 800 / 64 / 4,096 bytes respectively. That mode uses 200,000 messages for both the 1% and 5% loss bursts, plus 3,000 messages for the delay profile. Each comparison client has a 300-second limit, including long TCP loss bursts. Use a separate output file for each mode and set `TOKIO_WORKER_THREADS=4` for the worker count used in the latest validation.

## Concurrent proxy validation

Build `examples/concurrency.rs`, the echo benchmark server, and `example/proxy` in task copies with private caches. Start the server and proxy inside the same disposable network namespace, then run:

```sh
/path/to/task/target/release/examples/concurrency 127.0.0.1:19201 1024 30
```

The driver establishes connections with eight concurrent handshakes, keeps all connections open before sending, and validates 64 / 800 / 4,096-byte echoes with unique connection/message IDs. The receiving application pauses for 30 ms to exercise buffering. This is a transport workload, not 1,024 authenticated Minecraft players. The timeout is 120 seconds. Apply any loss rules only to the disposable namespace and inspect its UDP receive-buffer errors as well as netem counters.

## Concurrent throughput benchmark

Build `examples/concurrency_benchmark.rs` in the task copy. Inside the same private
network namespace as the echo server, run:

```sh
/path/to/task/target/release/examples/concurrency_benchmark 127.0.0.1:19132 1024 128 800
```

The arguments are address, connection count, messages per connection, and payload
size in bytes (at least 17). The driver uses eight concurrent handshakes and four
Tokio workers. Every connection first measures 20 sequential round trips. A
barrier then starts the throughput burst, with up to 16 messages in flight per
connection, refilling a slot after each echo rather than waiting for a whole batch.
Every ordered echo is checked against its connection and message ID.
Connections remain open until all bursts finish. The timeout is 180 seconds.

Setup time and RTT samples are excluded from the throughput timer. RTT percentiles
describe the preceding sequential phase, not delivery latency under the measured
burst. These synthetic sessions do not represent authenticated Minecraft players.
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
report median and range. These are synthetic transport connections, not Minecraft
players. Keep TCP's single-connection comparison separate from this workload.
