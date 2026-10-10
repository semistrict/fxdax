#!/usr/bin/env bash
# Benchmark fxdax against ext4 on virtio-blk (scripts/bench.sh) on a disposable
# x86-64 Linux KVM host, as root, writing the measurements, the guests' logs
# and the comparison to RESULTS.
#
# Usage: bench-linux-host.sh SOURCE RESULTS [ITERATIONS]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
iterations=${3:-3}
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"

# Runs build in one tree that persists between runs, so that they build only
# what changed.
tree="$work/bench-tree"
mkdir -p "$tree"
rsync -a --delete --exclude target --exclude /vendor/libkrun/test-prefix "$repo/" "$tree/"
repo=$tree

{
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    echo "cpus $(nproc)"
    grep MemTotal /proc/meminfo
    findmnt -no SOURCE,FSTYPE,OPTIONS /
    rustc --version
    podman --version
} > "$results/host.txt"

status=0
KRUNFW_LIB_DIR="$libkrunfw_lib" FXDAX_BENCH_RESULTS="$results" \
    "$repo/scripts/bench.sh" "$iterations" > "$results/comparison.md" 2> "$results/bench.log" ||
    status=$?
# The guests' root filesystems, copies of the Fedora image, and the filesystems'
# images are not results.
find "$results/logs" -type d -name rootfs -prune -exec rm -rf {} +
find "$results/logs" -type f -name '*.img' -delete
echo "Took ${SECONDS}s." | tee -a "$results/host.txt"
cat "$results/comparison.md"
exit "$status"
