# NetherNet signaling proxy

This example starts the library's `NetherNetProxy` API. It forwards only the Bedrock Dedicated Server's NetherNet HTTP signaling connection over TCP. It does not tunnel or proxy gameplay packets; after signaling, the Minecraft client establishes WebRTC directly to the backend server. The client therefore needs a reachable server ICE candidate.

Run it with:

```sh
cargo run --manifest-path example/nethernet_proxy/Cargo.toml -- \
  --listen 127.0.0.1:19144 --upstream 127.0.0.1:19142
```

Keep the official server on `transport=nethernet` at `127.0.0.1:19142`. This is separate from `example/proxy`, which forwards the legacy RakNet UDP transport.
