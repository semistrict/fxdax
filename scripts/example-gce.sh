#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs one of fxdax's examples, an optimized build, on a disposable x86-64 GCE
# host, optionally under perf: such as extentd-mapbench, which times the extent
# daemon's requests without a VM (TASK-26.18.3). Its output, the host and the
# source run go to the results directory (default:
# docs/measurements/gce-<example>-<time>), with what the run was for (note.md).
#
#   scripts/example-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_EXAMPLE names the example, FXDAX_EXAMPLE_ARGS its arguments (separated
# by spaces). With FXDAX_PERF=record, perf samples it, its threads' stacks, and
# perf.txt says where their time went. FXDAX_GCE_PROJECT is the project
# (default: gcloud's), FXDAX_GCE_ZONE the zone to try first (default:
# us-east4-a), FXDAX_BENCH_MACHINE the machine type (default: n2-standard-8),
# FXDAX_BENCH_NOTE what the run is for.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_BENCH_MACHINE:-n2-standard-8}
example=${FXDAX_EXAMPLE:?FXDAX_EXAMPLE names the example}
args=${FXDAX_EXAMPLE_ARGS:-}
perf=${FXDAX_PERF:-}
action=${1:-all}
instance=${2:-fxdax-bench-example-$(date -u +%Y%m%d-%H%M%S)}
results=${3:-$repo/docs/measurements/gce-$example-$(date -u +%Y-%m-%dT%H%MZ)}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-(bench|test)-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-bench- or fxdax-test- instance name.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_BENCH_MACHINE is a machine type' >&2; exit 2; }
[[ "$example" =~ ^[a-z0-9-]+$ ]] || { echo 'FXDAX_EXAMPLE names an example' >&2; exit 2; }
[[ "$args" =~ ^[A-Za-z0-9.\ -]*$ ]] || { echo 'FXDAX_EXAMPLE_ARGS are plain words' >&2; exit 2; }
[[ -z "$perf" || "$perf" == record ]] || { echo 'FXDAX_PERF is record, or unset' >&2; exit 2; }
# The host's purpose follows its name, so that the test and bench scripts' hosts serve too.
purpose=fxdax-$(echo "$instance" | cut -d- -f2)
lifetime=8h limit=${FXDAX_GCE_LIMIT:-2h}
create_flags=()
source "$repo/scripts/lib/gce.sh"

run() {
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    gce_wait_ready
    local run status=0
    run="run-$(date -u +%Y%m%dT%H%M%SZ)"
    gce_push_source "$run"
    gce_remote "sudo env FXDAX_PERF=$perf timeout --signal=TERM --kill-after=30s $limit \
        bash $run/source/scripts/lib/example-linux-host.sh $run/source $run/results $example $args" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    {
        echo "${FXDAX_BENCH_NOTE:-$example $args.}"
        echo
        echo "Machine: $machine, in $zone.${perf:+ Under perf $perf.}"
    } > "$results/note.md"
    cat "$results/output.txt" >&2 || true
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
