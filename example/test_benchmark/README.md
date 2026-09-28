# Network benchmark

Run both echo servers in separate terminals, then run each client with identical options. TCP and UDP can bind the same numeric port on one host. Use release mode for comparable performance numbers.

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

The loss runner requires Linux `unshare`, `ip`, and `tc`. Each run is bounded to 180 seconds.
