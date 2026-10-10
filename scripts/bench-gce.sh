#!/usr/bin/env bash
# Benchmark fxdax against ext4 on virtio-blk (scripts/bench.sh) on a disposable
# x86-64 GCE host with nested KVM (scripts/lib/gce.sh), and keep the results in
# the repo: the measurements, the host, the source measured, and the
# comparison of the filesystems (comparison.md).
#
#   create   the host
#   run      the benchmark on it, copying the results back
#   delete   the host, verified
#   all      create, run, and delete whatever happens
#
#   scripts/bench-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), FXDAX_BENCH_MACHINE the machine type
# (default: c3-standard-8), and FXDAX_BENCH_ITERATIONS how many times each
# workload runs (default: 3). The results go to
# docs/measurements/gce-fs-bench-<date> unless named.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_BENCH_MACHINE:-c3-standard-8}
iterations=${FXDAX_BENCH_ITERATIONS:-3}
action=${1:-all}
instance=${2:-fxdax-bench-$(date -u +%Y%m%d-%H%M%S)}
results=${3:-$repo/docs/measurements/gce-fs-bench-$(date -u +%Y-%m-%d)}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-bench-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-bench- instance name.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_BENCH_MACHINE is a machine type' >&2; exit 2; }
[[ "$iterations" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_BENCH_ITERATIONS is a count' >&2; exit 2; }
purpose=fxdax-bench
# The host deletes itself after this long; the run's own limit leaves time to
# copy the results back.
lifetime=8h limit=6h
create_flags=()
source "$repo/scripts/lib/gce.sh"

run() {
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    gce_wait_ready
    # Each run unpacks into a directory of its own, so that nothing of an
    # earlier run carries over.
    local run status=0
    run="run-$(date -u +%Y%m%dT%H%M%SZ)"
    gce_push_source "$run"
    gce_remote "sudo timeout --signal=TERM --kill-after=30s $limit \
        bash $run/source/scripts/lib/bench-linux-host.sh $run/source $run/results $iterations" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    tail -n 30 "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$status"
}

case "$action" in
    create) mkdir -p "$results"; gce_create ;;
    run) gce_locate; run ;;
    delete) gce_locate; gce_delete ;;
    all) mkdir -p "$results"; gce_all run ;;
    *) echo "Usage: $0 [all|create|run|delete] [instance] [results-directory]" >&2; exit 2 ;;
esac
