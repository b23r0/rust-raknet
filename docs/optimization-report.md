# Transport optimization validation

## Scope

The baseline is version 0.14.2, commit `5658a982d19f41014ddb8d78c2fdf5fca9314acb`. The candidate keeps the public RakNet API, supported reliability modes, default protocol version 10, protocol 11 support, the 64-datagram flight window, and the 50 ms initial/minimum retransmission timeout. No new wire extension is required.

Changes include 24-bit sequence wrap handling, reliable-frame deduplication, bounded ACK/NACK processing, shared retransmission payloads, bounded queues and asynchronous backpressure, prompt cancellation, offline-reply replay, IPv6 address encoding, and bounded NetherNet forwarding tasks. Common channel-zero traffic bypasses the channel map; contiguous ordered traffic bypasses the reorder map. Application sends avoid the outbound channel hop. The socket keeps only a weak UDP reference so listener shutdown can release its port.

The new receive and allocation limits intentionally disconnect excessive state and incomplete fragments after 60 seconds without progress. These limits are documented in the main README. They narrow previously unbounded behavior; they are not a claim of unlimited peer compatibility.

## Isolation and methodology

All edits, dependency caches, toolchains, builds, and test data used a task-specific copy under `/tmp`. Network tests ran inside separate user, PID, mount, and network namespaces. Only the namespace's loopback interface received `netem` settings. The host's interfaces, qdiscs, shell configuration, tools, and shared caches were not changed.

Baseline and candidate used the same Rust 1.98.1 compiler, resolved Tokio 1.53.1 dependency, release profile, and benchmark client. The candidate's minimum Tokio requirement is 1.21 for stable `JoinSet`; this comparison does not measure a Tokio upgrade. Builds and benchmarks ran sequentially. Processes used nice level 10 on an Intel Core i7-9700F (8 logical CPUs). The focused latency check pinned the server to CPU 0 and the client to CPU 7.

The echo benchmark runs `ReliableOrdered` against length-prefixed TCP. Throughput counts echoed application bytes per direction, not link utilization. Loopback MTU was 1500 with aggregation limits reduced to 1500 bytes/one segment. Loss applies independently to packets traversing loopback in both directions, including ACKs. WAN delay is 5 ms per traversal. Random loss is not a deterministic replay of the same packet losses.

Each throughput profile alternates baseline/candidate order over three repetitions. TCP has one control run per profile and is not a three-run median. Each run warms up with 100 round trips, records 300 RTT samples, then runs its pipelined burst. CPU is client user + system time plus server user + system time, including handshake and RTT phases. Memory is the sum of client/server peak RSS; these process peaks need not occur simultaneously.

For small latency differences, a separate check uses 1,000 warmups and 10,000 RTT samples per run, five paired repetitions, and separate fixed CPUs for client and server. Raw observations, timeouts and failed cases must remain visible; successful-run medians do not turn a failed profile into a pass.

## Compatibility and limits

Unit/integration coverage includes both supported protocol versions, all five reliability modes, ordering channels, fragmented payloads, reliable duplicates, sequence wraparound, malicious ACK ranges, receive/send budgets, bidirectional application backpressure, foreign UDP source rejection, lost offline replies, repeated construction/drop, and NetherNet task cancellation.

An independent peer pins `sandertv/go-raknet` v1.15.2. Its optional security cookies are disabled; this library does not negotiate cookies or encryption. The driver checks 400 unique ordered messages with 64, 800, 4,096, and 10,000-byte payloads. See [peer instructions](../tests/interop/README.md).

A known limitation remains: long fragmented-message stress with 5% loss from Rust to this Go server can hit the Go peer's 16-concurrent-split limit. Original 0.14.2 reproduced the same error. This is recorded as an interoperability failure, not a successful compatibility test. The separate Minecraft client/server gameplay flow was not repeated during this optimization task.

Cross-platform/MSRV CI changes are configuration only until GitHub Actions runs them. Local validation covers Linux with the compiler stated above. Clippy was unavailable in the isolated toolchain; no host component was installed.

## Measured results (2026-09-30)

Candidate: **18/18** throughput runs completed. Baseline: **17/18** completed; the second 1% loss run exited with `ConnectionClosed` after 62.2 seconds. The comparison runner correctly returned a nonzero status for that baseline failure. TCP: **6/6** controls completed. Failed runs are retained in the [raw comparison data](validation-2026-09-30/comparison.jsonl).

Throughput in MiB/s, median of successful runs (TCP is a single control):

| Profile | Payload / burst messages | Baseline | Candidate | TCP | Candidate / baseline |
|---|---|---:|---:|---:|---:|
| 0% loss | 800 B / 20,000 | 11.58 | 75.28 | 114.83 | 6.50x |
| 1% loss | 800 B / 20,000 | 12.79 | 74.94 | 19.89 | 5.86x |
| 5% loss | 800 B / 20,000 | 9.87 | 68.86 | 1.07 | 6.98x |
| Small packets, 0% loss | 64 B / 30,000 | 0.42 | 4.02 | 10.10 | 9.57x |
| Fragmented, 0% loss | 4,096 B / 5,000 | 9.72 | 86.64 | 262.76 | 8.91x |
| 1% loss + 5 ms each way | 800 B / 3,000 | 1.77 | 4.46 | 1.04 | 2.52x |

These gains include removal of baseline queue scans, copies, task hops and queue-induced drops; they are not a prediction for every workload. TCP remains faster in the clean and fragmented loopback controls. No change to congestion-control policy is claimed.

CPU seconds and aggregate peak RSS (KiB), same successful runs:

| Profile | CPU baseline → candidate | RSS baseline → candidate |
|---|---:|---:|
| 0% loss | 2.30 → 0.64 | 16,616 → 9,752 |
| 1% loss | 2.08 → 0.63 | 15,686 → 9,892 |
| 5% loss | 2.72 → 0.61 | 18,556 → 10,120 |
| Small packets, 0% loss | 6.80 → 0.81 | 11,448 → 8,472 |
| Fragmented, 0% loss | 3.16 → 0.61 | 33,616 → 9,808 |
| 1% loss + 5 ms each way | 0.29 → 0.16 | 11,964 → 8,696 |

### Focused latency check

The fixed-CPU check has 50,000 measured RTTs per revision, across five paired runs. Values below are medians of each run's percentile, not percentiles recomputed from all individual samples. [Raw latency observations](validation-2026-09-30/latency.jsonl).

| RTT metric (µs) | Baseline | Candidate |
|---|---:|---:|
| p50 | 25.6 | 25.6 |
| p95 | 48.7 | 43.3 |
| p99 | 200.2 | 128.1 |

The 300-sample profile checks contain noisy tails: clean p99 is 67.9 → 90.2 µs, and WAN p99 is 16.56 → 61.59 ms. Those observations are retained, not discarded. The larger clean sample above does not show a regression. Random-loss tail percentiles require a separate larger sample; the WAN follow-up is recorded below. Finite local measurements cannot guarantee performance on all hardware, load levels or network paths.

### Local validation

- 55 unit tests and 8 integration tests passed. [Test output](validation-2026-09-30/tests.log).
- Documentation: 1 compiled example passed; 15 existing ignored examples remain ignored. [Output](validation-2026-09-30/doc-tests.log).
- Formatting and all four example manifests passed their checks.
- The isolated benchmark runner passed smoke checks in all three modes (42/11/7 synthetic cases); these smoke outputs are not performance measurements.
- Independent-peer and old-version results are recorded below; failures are not included in the passing unit-test count.

### Binary identities

- Baseline benchmark SHA-256: `e3f270f6996799be2e7bb905c97acc5478228b19ea014bfdea5b0ecd74d5496e`.
- Candidate benchmark SHA-256: `0688f7b1caba8f5455959ab09b56e51cbc01ae47ef36e926e631ea713fc13d4f`.

### Interoperability results

| Peer combination | Network | Result |
|---|---|---|
| Candidate Rust client ↔ candidate Rust server | v10/v11, all five modes and ordering channels | Integration tests passed |
| Candidate ↔ original 0.14.2, both client/server directions | IPv4, 0% and 5% loss | 4/4 cases passed; 400 ordered payloads per case |
| Candidate ↔ Go v1.15.2, both directions | IPv4 and IPv6, 0% loss | 4/4 cases passed |
| Go client → candidate Rust server | IPv4 and IPv6, 5% loss | 2/2 cases passed |
| Candidate Rust client → Go server | IPv4 and IPv6, 5% loss | 2/2 cases failed: Go concurrent-split limit |

The Go failures are therefore a remaining boundary, not a claim that every RakNet implementation is compatible. [Independent-peer output](validation-2026-09-30/interop.log), [old-version cross-check](validation-2026-09-30/cross-baseline.log), and [original-version reproduction of the Go failure](validation-2026-09-30/baseline-go-failure.log) preserve the evidence. The legacy cross-check is IPv4; the new decoder additionally has a fixture for the old nonstandard IPv6 encoding.

## Reproduction

Build the unchanged baseline and candidate benchmark in separate disposable copies with matching compiler/dependency versions and private caches. From a task directory, run the provided isolation entry point against the two prebuilt binaries:

```sh
bash example/test_benchmark/compare_revisions.sh BASE_BINARY CANDIDATE_BINARY > comparison.jsonl
bash example/test_benchmark/compare_revisions.sh BASE_BINARY CANDIDATE_BINARY --latency > latency.jsonl
bash example/test_benchmark/compare_revisions.sh BASE_BINARY CANDIDATE_BINARY --wan-latency > wan-latency.jsonl
```

The entry point creates its own user/network namespace and copies the executables. It must be invoked from a context that permits namespace creation; there is no host-network fallback. The original comparison data used the committed entry point. The focused runs used an equivalent task-local driver with the same affinities, network settings and sample counts; the flags now expose those settings for repetition. Namespace or host scheduling differences can change the numbers.

## WAN latency follow-up

A separate fixed-CPU run used 2,000 measured RTTs after 100 warmups, three paired repetitions per revision, 1% random loss and 5 ms delay per direction. All six RakNet runs and the TCP control passed. [Raw follow-up data](validation-2026-09-30/wan-latency.jsonl).

| RTT metric (ms) | Baseline median [run range] | Candidate median [run range] |
|---|---:|---:|
| p50 | 10.451 [10.249–10.452] | 10.431 [10.319–10.447] |
| p95 | 12.729 [11.881–12.969] | 12.902 [12.575–13.226] |
| p99 | 83.805 [81.113–85.106] | 77.670 [74.768–91.765] |

The larger sample reverses the initial WAN p99 difference: candidate p99 is lower. Its p50 is also slightly lower; p95 is 0.173 ms higher, with overlapping run ranges. The random-loss measurements therefore do not establish a systematic tail-latency regression, but they also do not prove every percentile improves. Treat small changes as unresolved measurement variation until reproduced with more samples or controlled packet traces. The 50 ms minimum/initial RTO is unchanged.

Acceptance evidence is strongest for throughput, CPU, memory, clean median RTT, reliable delivery and the explicitly tested peer combinations. This report deliberately does not promise universal performance or complete Minecraft gameplay compatibility.
