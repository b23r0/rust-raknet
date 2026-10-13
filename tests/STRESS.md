# Sequence wrap and damaged-network stress tests

These checks exercise the production frame codec, ACK/NACK codec, send/receive queues and UDP sockets. They require a task-specific source copy, toolchain/cache/target directories and a private network namespace. Never execute the network soak on the host network.

## Short regressions

```sh
cargo test --release --lib arq::stress -- --nocapture
```

The regular regressions shuffle and duplicate real wire frames, introduce lost transmissions and timer retries, replay old ACKs, and cross a serial boundary under seeded random network damage. Every delivered ordered payload is checked against its full expected bytes and monotonic 64-bit application ID.

## Complete 24-bit epoch and virtual long-run network

```sh
cargo test --release --lib arq::stress -- --ignored --nocapture --test-threads=1
cargo test --release --all-features --lib arq::stress -- --include-ignored --nocapture --test-threads=1
```

The full-epoch case starts counters at zero and sends 16,777,280 messages plus a final continuation probe. It does not substitute a shortened counter or only initialize near `0xffffff`. Datagram, reliable and ordered indexes pass through their actual 24-bit wrap. Batches of 64 arrive in seeded random order, approximately 12.5% of received originals are duplicated, every 32nd window loses roughly one third of originals, and every eighth window withholds ACKs until a timeout retransmission. Over 4,096 retired ACK packets are retained and replayed twice against later live flight windows. Every message must be delivered once, in order, with its exact payload; outstanding bytes and receive queues must drain.

The virtual network runs four deterministic 24-hour timelines. It combines 45% independent data loss, 60% ACK/NACK packet loss, random data/NACK delays up to 4 seconds, ACK delays up to 20 seconds, duplicated data delayed up to an extra 20 seconds and duplicated ACKs delayed up to an extra 40 seconds. Thirty-second correlated outages recur every ten virtual minutes. The all-feature run uses its four seeds for send policy, recovery policy, both policies and neither policy. It begins near a real serial boundary, runs sender maintenance continuously and drains all remaining events after input stops. Virtual time accelerates queue/timer logic; it does not replace real socket/scheduler testing. Reassembly expiration uses the production wall clock, so fragmented-message lifetime is covered by the real UDP test instead.

## Real UDP soak

Inside an isolated Linux network, pass `TASK_HOST_NETNS` captured before entering the namespace:

```sh
# The default formal workload injects traffic for 600 seconds total, split
# between RakNet protocol 10 and 11, plus delivery/ACK drain time.
cargo test --release --test network_soak -- --ignored --nocapture
# A shorter diagnostic run, not the formal long-duration result:
RAKNET_SOAK_SECONDS=60 cargo test --release --test network_soak -- --ignored --nocapture
```

When compiled with send/recovery features, both sockets explicitly activate the corresponding preset/options; a short all-feature run checks this additional path. Both directions mix `Reliable` on channel 7 and `ReliableOrdered` on channel 3. Payloads are 800 bytes, with a 4,096-byte fragmented ordered message every 34 messages. The task-local UDP relay injects 35% independent packet loss, two-second correlated drop periods every thirty seconds, random ordinary packet delays of 0–150 ms, ACK delays of 400–2,500 ms and 20% packet duplication with up to four seconds additional delay. Handshake setup occurs before damage is enabled. No host qdisc or route is changed.

Each direction records the peer's actual accepted message count. Unordered delivery uses an exact ID set; ordered IDs must advance consecutively within their stream. Full payloads are validated, every expected ID must exist, and duplicate delivery is rejected immediately. After input stops, reliable ACKs are flushed and receivers remain active for eight seconds to catch late replay. Failure to drain, connection failure, payload corruption or any duplicate/gap fails the test.

## Interpreting results

Exact delivery applies to `Reliable` and `ReliableOrdered` while the tested connection remains live and the damaged path eventually recovers. Unreliable modes permit loss; sequenced modes may intentionally discard stale states, so applying a no-omission assertion to them would test the wrong contract.

Serial arithmetic and wire identifiers have finite ambiguity limits. These tests use bounded packet/feedback lifetimes well below half a 24-bit sequence epoch. They do not claim that an arbitrarily ancient packet retained across reused sequence IDs is distinguishable, or that a permanent outage can complete delivery. Report tested message counts, duration, seeds, actual injected drops/duplicates and residual queue state; keep execution logs outside source commits.
