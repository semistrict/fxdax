#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Benchmark fxdax against ext4 on virtio-blk (scripts/bench.sh) on a disposable
# x86-64 Linux KVM host, as root, writing the measurements, the guests' logs
# and the comparison to RESULTS.
#
# Usage: bench-linux-host.sh SOURCE RESULTS [ITERATIONS]
#
# FXDAX_BENCH_CASES and FXDAX_BENCH_GROUPS choose what runs (scripts/bench.sh).
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
iterations=${3:-3}
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"

# Runs build in one tree that persists between runs, so that they build only
# what changed.
tree="$work/bench-tree"
mkdir -p "$tree"
rsync -a --delete --exclude target --exclude /vendor/firecracker/build "$repo/" "$tree/"
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

# With FXDAX_BENCH_PERF=record, perf samples each case's VMM, its threads'
# stacks, and perf-CASE.txt says where their time went, or with record:EVENTS
# where the events happened; with stat:EVENTS, it counts the events into the
# cases' logs (test_fs_bench.rs).
if [[ -n ${FXDAX_BENCH_PERF:-} ]] && ! command -v perf > /dev/null; then
    apt-get -o DPkg::Lock::Timeout=600 install -y -qq linux-tools-common \
        "linux-tools-$(uname -r)" > /dev/null
fi

status=0
FXDAX_BENCH_RESULTS="$results" "$repo/scripts/bench.sh" "$iterations" > "$results/comparison.md" 2> "$results/bench.log" ||
    status=$?
for data in "$results"/logs/*/*/perf.data; do
    [[ -f $data ]] || continue
    case=$(basename "$(dirname "$data")")
    {
        echo "# By thread"
        perf report -i "$data" --stdio --no-children -g none --sort comm \
            --percent-limit 0.05 2>/dev/null
        echo "# By thread and symbol"
        perf report -i "$data" --stdio --no-children -g none --sort comm,dso,sym \
            --percent-limit 0.2 2>/dev/null
        echo "# Call graphs of the symbols over 1%, callers first"
        perf report -i "$data" --stdio --children -g caller,0.5,callee --sort sym \
            --percent-limit 1 2>/dev/null
    } > "$results/perf-$case.txt" || status=$?
    rm -f "$data"
done
# The guests' root filesystems, copies of the Alpine image (and their images,
# under Firecracker), and the filesystems' images and stores, which a failed
# case leaves, are not results.
find "$results/logs" -type d \( -name rootfs -o -name extents -o -name '*.img' \) -prune \
    -exec rm -rf {} +
find "$results/logs" -type f \( -name '*.img' -o -name rootfs.ext4 \) -delete
echo "Took ${SECONDS}s." | tee -a "$results/host.txt"
cat "$results/comparison.md"
exit "$status"
