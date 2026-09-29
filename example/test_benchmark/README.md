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

The client warms up with sequential request/echo rounds, measures full request/echo RTT percentiles, then sends the measured packet burst while receiving echoes concurrently. RakNet uses `ReliableOrdered`; TCP uses length-prefixed records and `TCP_NODELAY`. Reported MiB/s counts application payload in one direction, excluding protocol headers and acknowledgements. RTT includes both client and server processing and the local network path.

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

This runner creates its own isolated namespace. It alternates baseline/candidate order, runs three repetitions, and includes a TCP control for each profile: 800-byte clean/1%/5% loss, 64-byte small packets, 4 KiB fragmented messages, and 800-byte messages with 1% loss plus 5 ms delay per direction. Each result contains RTT percentiles, throughput, client/server CPU and peak RSS, raw benchmark output, and qdisc counters. Failures remain in the JSONL and cause a nonzero final exit status. Latency percentiles are sensitive to random loss; increase repetitions before treating a small difference as a regression. This measures one connection per process; it does not characterize high connection counts or Internet congestion fairness.

Measured results and compatibility boundaries are recorded in the [optimization validation report](../../docs/optimization-report.md).

For larger latency samples, add `--latency` (five pairs, 10,000 RTT samples each, no loss) or `--wan-latency` (three pairs, 2,000 samples each, 1% loss and 5 ms each way). These modes pin the client and server to opposite ends of the available CPU affinity list and require `taskset` and at least two available CPUs. They retain the TCP control and raw results. Use a separate output file for each mode.
