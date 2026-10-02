# Independent RakNet peer

This Go program pins `github.com/sandertv/go-raknet` v1.15.2. It is separate from the Rust test suite and exercises RakNet protocol 11 with payload identity checks, ordered delivery, and 64/800/4096/10000-byte messages. The Rust driver is `examples/interop.rs`.

Build and run both programs only inside a disposable task environment with task-local Rust/Go toolchains and caches and a private network namespace. Do not install toolchains or run loss injection on the host.

```sh
# Inside the isolated environment:
cargo build --release --example interop
cargo build --release --manifest-path examples/test_benchmark/Cargo.toml
(cd tests/interop && go build -o /task/bin/interop-peer .)

# Go server -> Rust client:
/task/bin/interop-peer server 127.0.0.1:19132
# In a second terminal in the same private namespace:
/task/target/release/examples/interop 127.0.0.1:19132

# Rust server -> Go client:
/task/target/release/test_benchmark --protocol raknet --type server --address 127.0.0.1:19132
/task/bin/interop-peer client 127.0.0.1:19132
```

Repeat with `[::1]:19132` for IPv6. The Go listener explicitly disables its optional security-cookie extension; this library does not negotiate cookies or encryption. A peer accepting protocol 11 requires `connect_with_version(..., 11)`; the default remains protocol 10.

These are transport checks, not Minecraft authentication or gameplay tests. Long fragmented-message stress against this Go version also exercises its strict concurrent-split limit; retain failures and inspect the peer log instead of treating a short successful echo as full compatibility. The performance report records the actual coverage and remaining limitations.
