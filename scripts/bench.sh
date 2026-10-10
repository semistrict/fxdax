#!/usr/bin/env bash
# Benchmark fxdax against the guest's own filesystem on this host: libkrun
# guests run the same workloads on fxdax's virtio-fs device, with DAX and
# without, on ext4 over a virtio-blk disk, and on libkrun's own virtio-fs
# device passing a host directory through (libkrun's tests/test_cases/src/
# test_fs_bench.rs), then scripts/bench-report.py compares them.
#
# Usage: scripts/bench.sh [ITERATIONS]   (default 3)
#
# Needs what scripts/test-e2e.sh needs, and podman to build the guests' Fedora
# root filesystem (on macOS, a running podman machine). The measurements and
# the guests' logs are kept under $FXDAX_BENCH_RESULTS (default: a new
# temporary directory).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
iterations=${1:-3}
results=${FXDAX_BENCH_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-bench.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)

FXDAX_BENCH_ITERATIONS=$iterations FXDAX_BENCH_RESULTS=$results \
    FXDAX_E2E_RESULTS=$results/logs \
    "$root/scripts/test-e2e.sh" bench-fs-ext4 bench-fs-fxdax-dax bench-fs-fxdax-nodax \
    bench-fs-passthrough >&2
echo "Results: $results" >&2
"$root/scripts/bench-report.py" "$results"
