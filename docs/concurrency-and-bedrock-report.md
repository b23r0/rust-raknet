# Minecraft compatibility and concurrent proxy validation

Date: 2026-09-30. Baseline: `21b53ab0b0b3f1be5e25acf444a8df0620e9893a` (0.14.2). Candidate: the source changes accompanying this report. No release version change or remote publication was requested.

## Isolation

All edits, builds and tests used `/tmp/codex-raknet-concurrency-3389irgc`, with separate baseline/candidate copies, private copied Rust 1.98.1 tools and Cargo dependencies, and task-specific HOME, CARGO_HOME, RUSTUP_HOME, CARGO_TARGET_DIR and XDG directories. Network tests ran in private user/network/PID namespaces. Only their loopback used MTU/GSO/GRO settings and netem. No host routes, qdiscs, sysctls, services, shared caches or shell configuration were changed. The retained Minecraft test installation was copied, not edited. Delivery to the target checkout is a reviewed commit; builds and tests are not repeated there.

Hardware: Intel Core i7-9700F, 8 logical CPUs, Linux x86_64. Release measurements used Tokio 1.53.1 and four workers per process. Affinity pins a process to a CPU but does not reserve it exclusively. Random loss traces differ between runs. The harness files retain their original task paths for provenance; use the repository benchmark entrypoint for reproduction.

## Changes and protocol boundaries

- Decode explicit IPv6 RakNet addresses independently of the sender's native AF_INET6 value. Linux Bedrock sends 10, Windows sends 23. A captured 188-byte accepted packet provides a regression fixture. Legacy encoding remains accepted.
- Respect Reply2's final negotiated MTU and reject impossible negotiation sizes. A relay test lowers it to 576 bytes and checks subsequent datagrams.
- Match sender and receiver's 65,536-fragment limit, reject oversized messages before allocating frames, and avoid an unnecessary split for an exact payload boundary.
- Replace flush's 5 ms polling with registered ACK notifications, including concurrent waiters and connection closure. This public API is not used to measure sequential benchmark RTT.
- Avoid MOTD copies for connected datagrams and queue inspection when debug logging is disabled. Parse owned payload bytes without first zero-filling a temporary allocation; this remains a copy, not end-to-end zero copy.
- Reserve an accept slot before acknowledging Request2. A full backlog waits for the peer's retry. The default pending backlog is 128 and can be configured; it is not a cap on active sessions.
- Request a 2 MiB receive buffer on the listener's own UDP socket. Existing OS limits can clamp it. Applications can request another size or supply a configured standard socket. No system-wide limit is changed. Process RSS measurements below exclude kernel socket memory.
- Forward both proxy directions independently, with a yield after eight ready messages, and limit upstream connection attempts to ten seconds. Preserve the client's RakNet protocol version upstream.

Reliability modes, ordering channels, ACK/NACK representation, retransmission policy and the 64-datagram reliable flight window remain compatible with the existing implementation. This statement does not certify every external peer or deployment.

## Correctness and interoperability

Final checks passed: **62 unit tests + 12 integration tests**, **1 proxy test**, and **1 executed doctest** (15 illustrative doctests remain ignored). All four standalone example projects checked successfully. Formatting checks passed. New regressions cover native IPv6 families/truncation, final MTU, fragment boundaries, concurrent flush/close, saturated acceptance with retries, 64 NetherNet clients under an eight-connection limit, and bidirectional forwarding under application backpressure.

The independent `sandertv/go-raknet v1.15.2` peer passed both client/server directions on IPv4 and IPv6, each verifying 400 uniquely identified ordered messages of 64, 800, 4,096 and 10,000 bytes. These four checks use 0% injected loss and DisableCookies=true. The previously documented long fragmented Rust-to-Go scenario at 5% loss is not newly certified; the baseline also encountered that peer's split limit. Existing local reliability/channel/version regressions passed.

Real official **Bedrock dedicated server 1.26.52.3**, RakNet 11 / application protocol 2193, passed direct and proxied handshakes plus RequestNetworkSettings. Both returned the same 14-byte NetworkSettings response. Before the native-family fix, both the baseline and initial candidate timed out on the accepted packet; the failure was not caused by the server's startup service-status message.

NetherNet direct and TCP signaling proxy endpoints both returned identical HTTP 200 responses for `/v1/join`. **NetherNetProxy forwards signaling only; WebRTC gameplay data remains direct and requires reachable ICE/TURN connectivity.** No authenticated retail-client login, world entry, movement or block manipulation was repeated this round. The synthetic concurrency test does not represent authenticated Minecraft players.

The benchmark runner's isolation guard, profile counts and equal TCP repetitions passed mock checks. Its actual isolated entrypoint emitted 54 long-throughput and 15 unpinned-latency synthetic records. Nested user-namespace creation was disallowed during the first smoke attempt; rerunning the entrypoint from its normal outer context succeeded with its own private namespace. No host-network fallback was used. Remote CI and other OS/MSRV matrix jobs were not executed locally.

## Concurrent RakNet reverse proxy

The client opens connections in groups of eight and waits until all are connected before sending. Each connection uses unique connection/message identifiers, rotates 64/800/4,096-byte payloads, delays application reads by 30 ms and verifies every ordered echo. The topology contains separate client, real proxy example and Rust echo-server processes. A 120-second timeout bounds each run. These are finite burst tests, not hour-long capacity or connection-churn certification.

Five paired 512-connection runs, 30 messages per connection:

| Metric (median) | Baseline | Candidate |
| --- | ---: | ---: |
| Transfer duration | 1.324 s | 0.845 s |
| Proxy CPU time | 1.88 s | 1.85 s |
| Proxy peak RSS | 36,636 KiB | 31,164 KiB |
| Namespace receive-buffer errors | 86,826 packets | 73,141 packets |

Transfer duration fell about **36.2%**. One longer paired test kept **1,024 connections** open and verified **307,200 ordered echoes**:

| Metric (one run) | Baseline | Candidate |
| --- | ---: | ---: |
| Transfer duration | 20.214 s | 18.883 s |
| Handshake duration | 11.318 s | 11.974 s |
| Proxy CPU time | 28.66 s | 26.58 s |
| Proxy peak RSS | 320,928 KiB | 301,852 KiB |
| Client peak RSS | 334,416 KiB | 340,612 KiB |
| Namespace receive-buffer errors | 4,844,369 packets | 3,648,496 packets |

The longer transfer improved about **6.6%**, with lower proxy CPU/RSS. Handshake time and client RSS did not improve in this single pair. A separate 1,024-connection/30-message run and 128-connection runs with 1% and 5% injected loss passed. Burst saturation still causes substantial kernel receive-buffer drops, even with 0% configured netem loss. Reliability recovered the tested messages; this is not evidence of zero kernel drops. Namespace counters include all test processes and setup/closure traffic, not just the proxy socket.

## Performance gate and repeat checks

The main README intentionally compares only RakNet and TCP. Internal baseline comparisons are retained here to evaluate changes, not as a public benchmark column. Both protocols now receive equal repetitions. Raw files retain all successful observations, CPU/RSS and qdisc statistics.

The final six-profile throughput series contained 54 successful observations (three repetitions each of baseline/candidate/TCP). Baseline-to-candidate throughput medians:

| Profile | Baseline | Candidate |
| --- | ---: | ---: |
| 800 B, 200,000 messages, clean | 75.90 MiB/s | 74.76 MiB/s |
| 64 B, 300,000 messages, clean | 6.51 MiB/s | 6.59 MiB/s |
| 4,096 B, 50,000 messages, clean | 113.15 MiB/s | 102.49 MiB/s |
| 800 B, 1% loss | 62.29 MiB/s | 78.91 MiB/s |
| 800 B, 5% loss | 50.96 MiB/s | 65.55 MiB/s |
| 800 B, 1% loss + 5 ms each way | 4.40 MiB/s | 4.43 MiB/s |

Because the three-run fragmented result was lower, an additional **ten repetitions per variant** were run without changing the Rust source. Fragmented throughput medians became **112.17 → 113.09 MiB/s**, with TCP at 265.12 MiB/s. The original lower measurements remain in the README/raw data; the additional check is not a replacement selected to hide them. A stable fragmentation regression was not reproduced.

Additional ten-run clean latency checks yielded these medians of run-level percentiles:

| Profile | Baseline p50 / p95 / p99 | Candidate p50 / p95 / p99 |
| --- | ---: | ---: |
| Separate CPU affinities | 25.85 µs / 38.65 µs / 67.50 µs | 25.25 µs / 38.90 µs / 66.75 µs |
| No CPU affinity | 20.50 µs / 34.10 µs / 65.65 µs | 19.55 µs / 36.05 µs / 66.55 µs |

The original delayed three-run check had p99 **75.704 → 85.924 ms** with independently randomized loss. Tail latency is not uniformly lower, and these samples do not establish a universal no-regression guarantee. High-concurrency improvements are measured; deployment-specific latency and capacity remain validation requirements. No larger flight window or more aggressive retransmission timeout was introduced to improve benchmark scores.

An initial draft using independent forwarding without bounded yields regressed the 512-connection transfer (1.172 → 1.430 s median) and was rejected. Intermediate batch-yield/parser results and the final complete series are retained under the raw-data directory. A test-only close/flush race was corrected by waiting for the server's ACK completion before the client closed; production behavior was not changed to mask it.

## Evidence

[Raw results, test logs, task harnesses and SHA-256 binary identities](validation-2026-09-30-concurrency/). Final measurement files are prefixed `buffer-`; `stress-buffer.jsonl`, `soak.jsonl`, `bedrock-final.jsonl`, `interop-final.jsonl` and the three `*-recheck.jsonl` files provide the compatibility/concurrency/repeat evidence. Historical optimization results remain in their earlier report.
