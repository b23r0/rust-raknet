#!/usr/bin/env bash
set -euo pipefail
if (( $# < 2 || $# > 3 )); then
    printf 'Usage: %s BASELINE_BINARY CANDIDATE_BINARY [--latency|--latency-unpinned|--wan-latency|--throughput-long]\nBuild both binaries in isolated task copies first.\n' "$0" >&2
    exit 2
fi
MODE=()
if (( $# == 3 )); then
    case $3 in
        --latency|--latency-unpinned|--wan-latency|--throughput-long) MODE=("$3") ;;
        *) printf 'Unknown comparison mode: %s\n' "$3" >&2; exit 2 ;;
    esac
    if [[ $3 == --latency || $3 == --wan-latency ]]; then
        command -v taskset >/dev/null || { printf 'taskset is required.\n' >&2; exit 1; }
    fi
fi
for binary in "$1" "$2"; do
    [[ -x "$binary" ]] || { printf 'Not executable: %s\n' "$binary" >&2; exit 1; }
done
for tool in bwrap ip tc python3; do
    command -v "$tool" >/dev/null || { printf 'Missing required tool: %s\n' "$tool" >&2; exit 1; }
done
[[ -x /usr/bin/time ]] || { printf 'GNU time is required.\n' >&2; exit 1; }
TASK_ROOT=$(mktemp -d /tmp/raknet-comparison-XXXXXXXX)
trap 'rm -rf -- "$TASK_ROOT"' EXIT
cp -- "$1" "$TASK_ROOT/base"
cp -- "$2" "$TASK_ROOT/work"
cp -- "$(dirname "${BASH_SOURCE[0]}")/compare_revisions.py" "$TASK_ROOT/compare.py"
PARENT_NETNS=$(readlink /proc/self/ns/net)
bwrap --die-with-parent --unshare-user --uid 0 --gid 0 --unshare-net --unshare-pid \
    --unshare-ipc --unshare-uts --cap-add CAP_NET_ADMIN \
    --ro-bind / / --tmpfs /home --tmpfs /run --tmpfs /tmp \
    --bind "$TASK_ROOT" "$TASK_ROOT" --proc /proc --dev /dev \
    --setenv RAKNET_PARENT_NETNS "$PARENT_NETNS" \
    --chdir "$TASK_ROOT" python3 "$TASK_ROOT/compare.py" "${MODE[@]}"
