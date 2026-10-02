# quic-go comparison

These echo drivers use [quic-go v0.63.0](https://github.com/quic-go/quic-go/tree/v0.63.0).
The module and dependency checksums are pinned here. Build only in a task copy
with a task-local Go toolchain, module cache and build cache:

```sh
cd examples/test_benchmark/quic
go build -o /path/to/task/bin/quic-single ./single
go build -o /path/to/task/bin/quic-concurrent ./concurrent
```

The measured build used Go 1.27.1. Populate dependencies inside the isolated
environment before running benchmarks; do not let Go install a toolchain into
the host cache.

Single-connection commands:

```sh
quic-single server ADDRESS
quic-single single ADDRESS MESSAGES PAYLOAD WARMUPS RTT_SAMPLES
```

Concurrent commands:

```sh
quic-concurrent server ADDRESS
quic-concurrent client ADDRESS CONNECTIONS MESSAGES PAYLOAD [--loaded-rtt]
```

Each connection uses one bidirectional reliable ordered stream, with four-byte
little-endian record lengths. TLS 1.3 encryption, congestion control and stream
flow control remain enabled. The local test uses a freshly generated certificate
and disables certificate verification on the benchmark client; this client is
not suitable for production.

Single-connection runs use a 1,400 B UDP packet budget and four Go workers.
Concurrent runs use a 1,372 B budget, four Go workers and four server UDP sockets
with `SO_REUSEPORT`. Path MTU discovery is disabled. Set `GOMAXPROCS=4` explicitly
and use the same client/server CPU sets as the Rust and C drivers.

The application windows are 64 messages for single-connection throughput and
16 per connection for concurrent throughput. Concurrent messages carry unique
connection and message IDs; every echoed payload and its order are checked.
Twenty sequential warmup echoes precede a common burst barrier. Connections
stay open until every burst finishes.

`--loaded-rtt` discards warmup RTTs and records every message's RTT during the
measured burst. It uses a 16-slot timestamp ring per connection and retains all
RTT samples for percentile reporting. Run it separately from ordinary throughput
so timestamping and sample storage do not affect the throughput comparison.
Use the parent's `measure_process.c` supervisor to collect whole-process peak
RSS for the client and server separately.
