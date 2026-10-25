#!/usr/bin/env bash
# Same-host comparison using frozen daemon/VMM binaries and one immutable guest kernel.
set -euo pipefail
repo=/var/lib/fxdax-test/bench-tree
variant=${1:?original or stage}
results=${2:?absolute results directory}
mkdir -p "$results"
source "$repo/scripts/lib/linux-host-setup.sh"
host_setup
case "$variant" in
    original)
        extentd_bin=/var/lib/fxdax-test/sqlite-gap-original-binaries/extentd
        firecracker=/var/lib/fxdax-test/sqlite-gap-original-binaries/firecracker
        ;;
    stage)
        extentd_bin=/var/lib/fxdax-test/sqlite-gap-stage-binaries/extentd
        firecracker=/var/lib/fxdax-test/sqlite-gap-stage-binaries/firecracker
        ;;
    *) exit 2 ;;
esac
export FXDAX_FIRECRACKER="$firecracker"
export FXDAX_GUEST_KERNEL=${FXDAX_GUEST_KERNEL:-/var/lib/fxdax-test/sqlite-gap-candidate1-vmlinux}
# Identical harness and guest-agent for both variants, from the current benchmark build.
export FXDAX_GUEST_AGENT="$repo/guest/tests/target/x86_64-unknown-linux-musl/debug/guest-agent"
export FXDAX_BENCH_ITERATIONS=3 FXDAX_BENCH_GROUPS=${FXDAX_BENCH_GROUPS:-sqlite} FXDAX_BENCH_RESULTS="$results"
export FXDAX_EXTENTD="$results/extentd.sock" FXDAX_EXTENTD_BIN="$extentd_bin"
control="$results/extentd.control"
sha256sum "$extentd_bin" "$firecracker" "$FXDAX_GUEST_KERNEL" "$FXDAX_GUEST_AGENT" \
    "$repo/guest/tests/target/debug/runner" > "$results/binaries.txt"
host_record > "$results/host.txt"
env FXDAX_EXTENTD_REMOTE=memory:///e2e FXDAX_EXTENTD_TOKEN=1 \
    FXDAX_EXTENTD_STATS="$results/extentd-stats.txt" FXDAX_EXTENTD_GRPC="$control" \
    "$extentd_bin" "$FXDAX_EXTENTD" > "$results/extentd.log" 2>&1 &
daemon=$!
trap 'kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; rm -f "$FXDAX_EXTENTD" "$control"' EXIT
while [[ ! -S $FXDAX_EXTENTD || ! -S $control ]]; do
    kill -0 "$daemon" || exit 1
    sleep 0.1
done
status=0
host_need perf
perf record -a --call-graph dwarf --freq 499 -o "$results/perf.data" -- \
    "$repo/guest/tests/target/debug/runner" test --test-case bench-fs-fxfs-guest-dax \
    --base-dir "$results/logs/bench-fs-fxfs-guest-dax" --keep-all > "$results/bench.log" 2>&1 || status=$?
perf report --stdio --percent-limit 0.03 -i "$results/perf.data" > "$results/perf.txt"
perf report --stdio --no-children --sort comm,symbol --percent-limit 0.03 -i "$results/perf.data" > "$results/perf-self.txt"
perf report --stdio --no-children --no-call-graph --sort comm --percent-limit 0 -i "$results/perf.data" > "$results/perf-commands.txt"
perf report --stdio --no-children --no-call-graph --sort comm,symbol --percent-limit 0 -i "$results/perf.data" > "$results/perf-all-self.txt"
"$repo/scripts/bench-report.py" "$results" > "$results/comparison.md" || status=$?
find "$results/logs" -type d \( -name rootfs -o -name extents -o -name '*.img' \) -prune -exec rm -rf {} +
find "$results/logs" -type f \( -name '*.img' -o -name rootfs.ext4 \) -delete
exit "$status"
