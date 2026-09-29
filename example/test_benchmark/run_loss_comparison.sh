#!/usr/bin/env bash
set -euo pipefail

# Require a binary built in an isolated development environment. Never build on the host.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BENCHMARK="${RAKNET_BENCHMARK:-$REPO_ROOT/example/test_benchmark/target/release/test_benchmark}"
if (( $# != 0 )); then
    printf 'This runner accepts no worker or namespace-bypass arguments.\n' >&2
    exit 2
fi
if [[ ! -x "$BENCHMARK" ]]; then
    printf 'Build the benchmark in an isolated environment and set RAKNET_BENCHMARK to its executable.\n' >&2
    exit 1
fi
for tool in bwrap ip tc timeout; do
    command -v "$tool" >/dev/null || { printf 'Missing required tool: %s\n' "$tool" >&2; exit 1; }
done
TASK_ROOT="$(mktemp -d /tmp/raknet-benchmark-XXXXXXXX)"
trap 'rm -rf -- "$TASK_ROOT"' EXIT
cp -- "$BENCHMARK" "$TASK_ROOT/benchmark"
HOST_NETNS="$(readlink /proc/self/ns/net)"

# Only the disposable task directory is writable. No user files are visible.
bwrap --die-with-parent --unshare-user --uid 0 --gid 0 --unshare-net --unshare-pid \
    --unshare-ipc --unshare-uts --cap-add CAP_NET_ADMIN \
    --ro-bind / / --tmpfs /home --tmpfs /run --tmpfs /tmp \
    --bind "$TASK_ROOT" "$TASK_ROOT" --proc /proc --dev /dev \
    --setenv RAKNET_PARENT_NETNS "$HOST_NETNS" \
    --chdir "$TASK_ROOT" bash --noprofile --norc -s -- "$TASK_ROOT/benchmark" <<'WORKER'
set -euo pipefail
[[ "$(readlink /proc/self/ns/net)" != "$RAKNET_PARENT_NETNS" ]] || exit 1
BENCHMARK=$1
ip link set lo up
ip link set dev lo mtu 1500 gso_max_size 1500 gso_max_segs 1 gro_max_size 1500
"$BENCHMARK" --protocol tcp --type server --address 127.0.0.1:19132 >/dev/null 2>&1 &
TCP_SERVER_PID=$!
"$BENCHMARK" --protocol raknet --type server --address 127.0.0.1:19132 >/dev/null 2>&1 &
RAKNET_SERVER_PID=$!
cleanup() {
    kill "$TCP_SERVER_PID" "$RAKNET_SERVER_PID" 2>/dev/null || true
    wait "$TCP_SERVER_PID" "$RAKNET_SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
sleep 0.5
run_case() {
    local loss_rate=$1 protocol=$2 run=$3
    tc qdisc replace dev lo root netem limit 100000 loss "${loss_rate}%"
    printf '\nloss=%s%% protocol=%s run=%s\n' "$loss_rate" "$protocol" "$run"
    timeout 180s "$BENCHMARK" --protocol "$protocol" --type client \
        --address 127.0.0.1:19132 --packets 20000 --payload-size 800 \
        --warmup 200 --latency-samples 300
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
WORKER
