#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Benchmark fxdax against ext4 on virtio-blk (scripts/bench.sh) on a kept
# x86-64 GCE host with nested KVM (scripts/lib/gce.sh), and keep the results in
# the repo: the measurements, the host, the source measured, the comparison of
# the filesystems (comparison.md) and what the run was for (note.md), and
# brings docs/measurements/fs-bench-history.md up to date
# (scripts/bench-history.py).
#
#   all      acquire a kept host, run, and stop it whatever happens
#   create   acquire a kept host and hold it for runs, until its stop
#   run      the benchmark on a held host, copying the results back
#   stop     the host, which ends its holding
#   delete   the host, verified: one broken beyond repair (gce_main)
#
#   scripts/bench-gce.sh [all|create] [results-directory]
#   scripts/bench-gce.sh run|stop|delete instance [results-directory]
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), FXDAX_BENCH_MACHINE the machine type
# (default: c3-standard-8), FXDAX_BENCH_CPU_PLATFORM its least CPU platform, if
# any (such as 'Intel Cascade Lake': test-gce.sh's hosts are so made, and a bench
# run may take a stopped one of them), and FXDAX_BENCH_ITERATIONS how many times each
# workload runs (default: 3). FXDAX_BENCH_CASES and FXDAX_BENCH_GROUPS choose
# what runs (scripts/bench.sh). FXDAX_BENCH_PERF=record profiles each case's
# VMM with perf, into perf-CASE.txt, record:EVENTS records the events there
# instead, and stat:EVENTS counts the events each 100 ms, into the cases' logs
# (test_fs_bench.rs). FXDAX_BENCH_GUEST_PERF, if set, profiles the guest's
# kernel through each workload, into the cases' logs. FXDAX_BENCH_FILES and
# FXDAX_BENCH_PGBENCH_SECS and FXDAX_BENCH_PGBENCH_SCALE size the metadata workloads and
# pgbench, and FXDAX_BENCH_VCPUS and FXDAX_BENCH_JOBS the VMs' vCPUs and the scaling group's
# jobs (test_fs_bench.rs).
# FXDAX_EXTENTD_REMOTE, if set, names the bucket the extent daemon keeps the
# stores' branches in (gs://bucket/prefix: a host made with FXDAX_GCE_STORAGE
# set can reach it, scripts/lib/gce.sh), and FXDAX_EXTENTD_CACHE the directory
# it keeps what it reads from there in. FXDAX_ZSTD_LEVEL is the zstd level stores
# compress their parts at (default 1; 0 stores them raw), to measure others.
# FXDAX_REMOTE_LATENCY_MS adds that many milliseconds to each of the bucket's
# requests (extent_remote's object_store_at), as a farther object store would.
# FXDAX_GUEST_CMDLINE adds to the guest kernel's arguments (such as
# cpuidle_haltpoll.force=1), to measure them. FXDAX_GUEST_KERNEL_BUILD (release or
# debug) builds the guest kernel from the source on the host. FXDAX_GCE_LIMIT caps
# a run's time on the host (timeout's DURATION, default 6h).
# FXDAX_BENCH_NOTE says what the run is for. The results go to
# docs/measurements/gce-fs-bench-<time>-<mark>, the run's mark
# (scripts/lib/gce.sh), unless named. Every other FXDAX_* and
# PJDFSTEST_* variable set here reaches the host too (scripts/lib/gce.sh), such
# as FXDAX_BENCH_WATCHDOG_SECS.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_BENCH_MACHINE:-c3-standard-8}
iterations=${FXDAX_BENCH_ITERATIONS:-3}
results=$repo/docs/measurements/gce-fs-bench-$(date -u +%Y-%m-%dT%H%MZ)-$gce_maker
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_BENCH_MACHINE is a machine type' >&2; exit 2; }
[[ "$iterations" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_BENCH_ITERATIONS is a count' >&2; exit 2; }
limit=${FXDAX_GCE_LIMIT:-6h}
create_flags=()
if [[ -n ${FXDAX_BENCH_CPU_PLATFORM:-} ]]; then
    create_flags=(--min-cpu-platform="$FXDAX_BENCH_CPU_PLATFORM")
fi

run() {
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    gce_wait_ready
    # Each run unpacks into a directory of its own, so that nothing of an
    # earlier run carries over.
    local run status=0
    run=$(gce_run_name)
    gce_push_source "$run"
    gce_remote_detached "$run" "$(gce_host_command "$run" bench-linux-host.sh "$iterations")" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    {
        echo "${FXDAX_BENCH_NOTE:-}"
        echo
        echo "Iterations: $iterations. Cases: ${FXDAX_BENCH_CASES:-all}." \
            "Workloads: ${FXDAX_BENCH_GROUPS:-all}.${FXDAX_BENCH_PERF:+ perf: $FXDAX_BENCH_PERF.}" \
            "${FXDAX_BENCH_GUEST_PERF:+Guest kernel profiled.}"
    } > "$results/note.md"
    "$repo/scripts/bench-history.py" >&2 || status=$?
    cat "$results/comparison.md" >&2
    echo "Results: $results" >&2
    return "$status"
}

gce_main "$@"
