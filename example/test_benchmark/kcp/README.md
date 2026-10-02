# Official C KCP comparison

These Linux benchmark adapters call the [upstream C implementation](https://github.com/skywind3000/kcp) directly. They
are test programs, not a Rust KCP binding or an alternative implementation.

Use upstream `skywind3000/kcp` commit
`b1a7a2101dcbb96017681a500d6b82bbe5a88766`. Keep its `ikcp.c`, `ikcp.h` and license
unchanged in the task directory. Build and run only in a disposable container,
VM or task copy with private caches and a private network namespace.

```sh
cc -O3 -DNDEBUG -std=c11 -I /path/to/task/kcp \
  benchmark.c /path/to/task/kcp/ikcp.c -o /path/to/task/kcp-benchmark
cc -O3 -DNDEBUG -std=c11 -pthread -I /path/to/task/kcp \
  concurrency.c /path/to/task/kcp/ikcp.c -o /path/to/task/kcp-concurrency
```

The single-connection adapter accepts the same `--type`, `--address`,
`--packets`, `--payload-size`, `--warmup` and `--latency-samples` options as the
Rust echo benchmark. Pass `--protocol kcp`.

The concurrent adapter takes `server|client ADDRESS CONNECTIONS MESSAGES PAYLOAD`.
It uses four pthread workers, four server receive sockets with `SO_REUSEPORT`,
and one distinct client UDP socket per connection. The Rust comparison uses four
Tokio workers and four server receive sockets too. The concurrent TCP control
uses `concurrency_benchmark --tcp-server ADDRESS` and
`concurrency_benchmark --tcp ADDRESS CONNECTIONS MESSAGES PAYLOAD`, with four
Tokio workers, four Linux `SO_REUSEPORT` listeners, `TCP_NODELAY` and reused
length-prefixed record buffers. Pin the server and client to
separate sets of four CPUs; affinity does not reserve these CPUs exclusively.

All three concurrent drivers measure 20 sequential RTTs per connection, then start
throughput at a common barrier. Each connection keeps a sliding window of up to
16 messages in flight, refilling it after each echo. Every echo is checked for its
connection ID, message ID and payload; all connections remain open until the
burst finishes. RTT samples and setup are excluded from throughput. KCP has no
connection handshake, so setup durations are not comparable.

KCP settings are message mode, send window 64, receive window 128, UDP MTU 1,400
bytes, `ikcp_nodelay(kcp, 1, 10, 2, 1)`, and immediate flush after each write/input.
No FEC or encryption is enabled. `rust-raknet` uses its normal `ReliableOrdered`,
64-datagram flight window and retry timings. For the README's single-connection
comparison, pass `--raknet-mtu 1428` to both Rust processes: nominal `rust-raknet` MTU
includes 28 bytes of IPv4/UDP overhead, while KCP's UDP MTU excludes it. Both
therefore have a 1,428-byte IPv4 packet budget. The application window is 64
messages for all three single-connection drivers, including TCP. The C adapter
uses one event loop per process; the Rust drivers use four Tokio workers.

The concurrent comparison uses the ordinary nominal `rust-raknet` MTU of 1,400 bytes
and KCP UDP MTU of 1,400 bytes. Their physical IPv4 budgets differ by 28 bytes;
the 800-byte payloads fit in one datagram for both protocols. Do not extrapolate
these results to fragmented concurrent workloads. An additional equal-MTU
control can lower only KCP's UDP MTU to 1,372 bytes and label it separately.

Repeat and alternate protocol order. Record namespace-local UDP receive/send
buffer errors as well as netem counters, including at 0% injected loss. Random
loss affects data and ACKs in both directions, and traces differ between runs.
Never apply `tc`, interface or network configuration to the host.
