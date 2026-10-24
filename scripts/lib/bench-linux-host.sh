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
# FXDAX_GUEST_KERNEL_BUILD (release or debug) builds the guest kernel from SOURCE.
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
iterations=${3:-3}
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
host_tree bench-tree
# Build the guest kernel from this source when asked, as test-linux-host.sh does.
case ${FXDAX_GUEST_KERNEL_BUILD:-} in
    '') ;;
    release | debug)
        kernel_work="$work/guest-kernel-$FXDAX_GUEST_KERNEL_BUILD"
        kernel_debug=0
        [[ $FXDAX_GUEST_KERNEL_BUILD == debug ]] && kernel_debug=1
        channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$repo/guest/rust/rust-toolchain.toml")
        rustup toolchain install "$channel" --component rust-src --profile minimal > /dev/null
        env -u RUSTUP_TOOLCHAIN FXDAX_GUEST_KERNEL_WORK="$kernel_work" \
            FXDAX_GUEST_KERNEL_DEBUG=$kernel_debug "$repo/scripts/build-guest-kernel.sh" x86_64 \
            > "$results/guest-kernel.log" 2>&1
        export FXDAX_GUEST_KERNEL="$kernel_work/out/x86_64/vmlinux"
        sha256sum "$FXDAX_GUEST_KERNEL" > "$results/guest-kernel.txt"
        ;;
    *) echo "FXDAX_GUEST_KERNEL_BUILD is release or debug" >&2; exit 2 ;;
esac
# With FXDAX_BENCH_PERF=record, perf samples each case's VMM, its threads'
# stacks (record-all: the whole host's, the extent daemon's too), and
# perf-CASE.txt says where their time went, and perf-CASE-THREAD.txt each busy
# thread's (scripts/lib/perf-report.sh), or with record:EVENTS
# where the events happened; with stat:EVENTS, it counts the events into the
# cases' logs (test_fs_bench.rs).
if [[ -n ${FXDAX_BENCH_PERF:-} ]]; then
    host_need perf
fi

{
    host_record
    findmnt -no SOURCE,FSTYPE,OPTIONS /
    podman --version
} > "$results/host.txt"

status=0
FXDAX_BENCH_RESULTS="$results" "$repo/scripts/bench.sh" "$iterations" > "$results/comparison.md" 2> "$results/bench.log" ||
    status=$?
source "$(dirname "${BASH_SOURCE[0]}")/perf-report.sh"
for data in "$results"/logs/*/*/perf.data; do
    [[ -f $data ]] || continue
    case=$(basename "$(dirname "$data")")
    perf_report "$data" "$results/perf-$case" || status=$?
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
