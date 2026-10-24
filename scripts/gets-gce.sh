#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure ranged GETs from GCS on a disposable x86-64 GCE host (TASK-28.11;
# doc-7, decision 9): fxdax-gets puts an object in the bucket, GETs parts of it
# by part size and GETs in flight, and deletes it. The table, the host and the
# source measured go to docs/measurements/gce-gets-<time>-<mark>, the run's
# mark (scripts/lib/gce.sh), unless named, with what the run was for (note.md).
#
#   scripts/gets-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_GETS_BUCKET names the bucket and prefix (gs://bucket/prefix), which the
# run puts its object under; FXDAX_GCE_PROJECT is the project (default:
# gcloud's), FXDAX_GCE_ZONE the zone to try first (default: us-east4-a, the
# bucket's region), FXDAX_BENCH_MACHINE the machine type (default:
# c3-standard-8, the benchmarks'), FXDAX_GETS_MIB the object's size (default
# 512) and FXDAX_GETS_SECS the seconds each measurement takes (default 10).
# FXDAX_BENCH_NOTE says what the run is for. Every other FXDAX_* and
# PJDFSTEST_* variable set here reaches the host too (scripts/lib/gce.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
# The host reads and writes the bucket with the project's default service account.
FXDAX_GCE_STORAGE=1
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_BENCH_MACHINE:-c3-standard-8}
bucket=${FXDAX_GETS_BUCKET:?FXDAX_GETS_BUCKET names the bucket: gs://bucket/prefix}
mib=${FXDAX_GETS_MIB:-512}
secs=${FXDAX_GETS_SECS:-10}
action=${1:-all}
instance=${2:-fxdax-bench-gets-$(date -u +%Y%m%d-%H%M%S)-$gce_maker}
results=${3:-$repo/docs/measurements/gce-gets-$(date -u +%Y-%m-%dT%H%MZ)-$gce_maker}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-bench-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-bench- instance name.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_BENCH_MACHINE is a machine type' >&2; exit 2; }
[[ "$bucket" =~ ^gs://[a-z0-9._-]+(/[A-Za-z0-9._/-]*)?$ ]] || { echo 'FXDAX_GETS_BUCKET is gs://bucket/prefix' >&2; exit 2; }
[[ "$mib" =~ ^[1-9][0-9]*$ && "$secs" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_GETS_MIB and FXDAX_GETS_SECS are counts' >&2; exit 2; }
purpose=fxdax-bench
lifetime=8h limit=${FXDAX_GCE_LIMIT:-2h}
create_flags=()

run() {
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    gce_wait_ready
    local run status=0
    run=$(gce_run_name)
    gce_push_source "$run"
    gce_remote_detached "$run" \
        "$(gce_host_command "$run" gets-linux-host.sh "$bucket/$run" "$mib" "$secs")" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    {
        echo "${FXDAX_BENCH_NOTE:-Ranged GETs from GCS (TASK-28.11).}"
        echo
        echo "Machine: $machine, in $zone. Bucket: $bucket."
    } > "$results/note.md"
    cat "$results/gets.md" >&2 || true
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
