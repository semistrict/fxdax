#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Benchmark fxdax against ext4 on virtio-blk (scripts/bench.sh) on an x86-64 Linux host of one's
# own with KVM, over ssh (FXDAX_HOST, ubu1 by default; scripts/lib/host.sh), as a measurement,
# alone there and on CPUs of its own (fxjob --bench), and keep the results in the repo: the
# measurements, the host, the source measured, the comparison of the filesystems (comparison.md)
# and what the run was for (note.md); and bring docs/measurements/fs-bench-history.md up to date
# (scripts/bench-history.py).
#
#   scripts/bench-host.sh [RESULTS]
#   scripts/bench-host.sh resume RUN RESULTS | cancel RUN
#
# The results go to docs/measurements/HOST-fs-bench-<time>-<mark>, the run's mark
# (scripts/lib/host.sh), unless named. FXDAX_BENCH_ITERATIONS is how many times each workload
# runs (default: 3). FXDAX_BENCH_CASES and FXDAX_BENCH_GROUPS choose what runs (scripts/bench.sh).
# FXDAX_BENCH_PERF=record profiles each case's VMM with perf, into perf-CASE.txt, record:EVENTS
# records the events there instead, and stat:EVENTS counts the events each 100 ms, into the cases'
# logs (test_fs_bench.rs). FXDAX_BENCH_GUEST_PERF, if set, profiles the guest's kernel through each
# workload, into the cases' logs. FXDAX_BENCH_FILES and FXDAX_BENCH_PGBENCH_SECS and
# FXDAX_BENCH_PGBENCH_SCALE size the metadata workloads and pgbench, and FXDAX_BENCH_VCPUS and
# FXDAX_BENCH_JOBS the VMs' vCPUs and the scaling group's jobs (test_fs_bench.rs).
# FXDAX_EXTENTD_REMOTE, if set, names the bucket the extent daemon keeps the stores' branches in
# (an object store's URL the host reaches), and FXDAX_EXTENTD_CACHE the directory it keeps what it
# reads from there in. FXDAX_ZSTD_LEVEL is the zstd level stores compress their parts at (default
# 1; 0 stores them raw), to measure others. FXDAX_REMOTE_LATENCY_MS adds that many milliseconds to
# each of the bucket's requests (extent_remote's object_store_at), as a farther object store would.
# FXDAX_GUEST_CMDLINE adds to the guest kernel's arguments (such as cpuidle_haltpoll.force=1), to
# measure them. FXDAX_GUEST_KERNEL_BUILD (release or debug) builds the guest kernel from the source
# on the host. FXDAX_HOST_LIMIT caps the run's time on the host (timeout's DURATION, default 6h).
# FXDAX_BENCH_NOTE says what the run is for. Every other FXDAX_* and PJDFSTEST_* variable set here
# reaches the host too (scripts/lib/host.sh), such as FXDAX_BENCH_WATCHDOG_SECS.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
iterations=${FXDAX_BENCH_ITERATIONS:-3}
[[ "$iterations" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_BENCH_ITERATIONS is a count' >&2; exit 2; }
limit=6h
host_bench=1

# finish STATUS writes what the run was for, brings the history up to date, and says how the run
# went: STATUS, unless the history failed.
finish() {
    local status=$1
    {
        echo "${FXDAX_BENCH_NOTE:-}"
        echo
        echo "Iterations: $iterations. Cases: ${FXDAX_BENCH_CASES:-all}." \
            "Workloads: ${FXDAX_BENCH_GROUPS:-all}.${FXDAX_BENCH_PERF:+ perf: $FXDAX_BENCH_PERF.}" \
            "${FXDAX_BENCH_GUEST_PERF:+Guest kernel profiled.}"
    } > "$results/note.md"
    "$repo/scripts/bench-history.py" >&2 || status=$?
    if [[ -f $results/comparison.md ]]; then cat "$results/comparison.md" >&2; fi
    echo "Results: $results" >&2
    return "$status"
}
host_after=finish
host_actions "$@"
(( $# <= 1 )) || { echo "Usage: $0 [RESULTS] | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=${1:-$repo/docs/measurements/$(host_label)-fs-bench-$(date -u +%Y-%m-%dT%H%MZ)-$host_mark}

status=0
host_start
echo "Results: $results" >&2
host_job bench-linux-host.sh "$iterations" > "$results/remote.log" 2>&1 || status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
