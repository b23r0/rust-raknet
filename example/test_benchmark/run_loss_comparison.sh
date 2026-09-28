#!/usr/bin/env bash
set -euo pipefail

# All traffic shaping runs inside a new network namespace; there is no host-network fallback.
SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "$SCRIPT_PATH")/../.." && pwd)"
BENCHMARK="$REPO_ROOT/example/test_benchmark/target/release/test_benchmark"

if [[ "${1:-}" != "--isolated-worker" ]]; then
    if [[ ! -x "$BENCHMARK" ]]; then
        cargo build --release --manifest-path "$REPO_ROOT/example/test_benchmark/Cargo.toml"
    fi
    exec unshare --user --map-root-user --net "$SCRIPT_PATH" --isolated-worker
fi

ip link set lo up
ip link set dev lo mtu 1500 gso_max_size 1500 gso_max_segs 1 gro_max_size 1500
tc qdisc add dev lo root netem limit 100000 loss 0%

"$BENCHMARK" --protocol tcp --type server --address 127.0.0.1:19132 >/dev/null 2>&1 &
TCP_SERVER_PID=$!
"$BENCHMARK" --protocol raknet --type server --address 127.0.0.1:19132 >/dev/null 2>&1 &
RAKNET_SERVER_PID=$!

cleanup() {
    kill "$TCP_SERVER_PID" "$RAKNET_SERVER_PID" 2>/dev/null || true
    wait "$TCP_SERVER_PID" "$RAKNET_SERVER_PID" 2>/dev/null || true
    tc qdisc del dev lo root 2>/dev/null || true
}
trap cleanup EXIT INT TERM
sleep 0.5

run_case() {
    local loss_rate="$1"
    local protocol="$2"
    local run="$3"

    tc qdisc del dev lo root
    tc qdisc add dev lo root netem limit 100000 loss "${loss_rate}%"
    printf '\n=== loss=%s%% protocol=%s run=%s ===\n' "$loss_rate" "$protocol" "$run"
    timeout 180s "$BENCHMARK" \
        --protocol "$protocol" \
        --type client \
        --address 127.0.0.1:19132 \
        --packets 20000 \
        --payload-size 800 \
        --warmup 200 \
        --latency-samples 300
    tc -s qdisc show dev lo
}

profile_index=0
for loss_rate in 0 1 5; do
    for run in 1 2 3; do
        if (( (profile_index + run) % 2 == 1 )); then
            run_case "$loss_rate" tcp "$run"
            run_case "$loss_rate" raknet "$run"
        else
            run_case "$loss_rate" raknet "$run"
            run_case "$loss_rate" tcp "$run"
        fi
    done
    profile_index=$((profile_index + 1))
done
