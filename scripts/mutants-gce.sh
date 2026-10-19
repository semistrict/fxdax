#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing (scripts/mutants.sh, TASK-44.1) on a disposable x86-64 GCE
# host, where fxdax's Linux-only code (the extent daemon, the DAX window's
# userfaultfd) builds and its tests run. Each crate's report comes back to the
# results directory (default: docs/measurements/gce-mutants-<time>).
#
#   scripts/mutants-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_MUTANTS_CRATES names the crates, separated by spaces (default:
# scripts/mutants.sh's), FXDAX_MUTANTS_SHARD the shard (K/N) and
# FXDAX_MUTANTS_JOBS the mutants at once (default: a third of the host's CPUs);
# FXDAX_MUTANTS_LIST a file in the repository listing the mutants to run alone
# (scripts/mutants.sh).
# A host lives 8 hours: shard a sweep longer than its limit (FXDAX_GCE_LIMIT,
# default 7h). FXDAX_GCE_PROJECT is the project (default: gcloud's),
# FXDAX_GCE_ZONE the zone to try first (default: us-east4-a) and
# FXDAX_MUTANTS_MACHINE the machine type (default: n2-standard-8).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_MUTANTS_MACHINE:-n2-standard-8}
crates=${FXDAX_MUTANTS_CRATES:-}
shard=${FXDAX_MUTANTS_SHARD:-}
jobs=${FXDAX_MUTANTS_JOBS:-}
list=${FXDAX_MUTANTS_LIST:-}
action=${1:-all}
instance=${2:-fxdax-test-mutants-$(date -u +%Y%m%d-%H%M%S)}
results=${3:-$repo/docs/measurements/gce-mutants-$(date -u +%Y-%m-%dT%H%MZ)}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-test-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-test- instance name.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_MUTANTS_MACHINE is a machine type' >&2; exit 2; }
[[ "$crates" =~ ^[a-z_\ ]*$ ]] || { echo 'FXDAX_MUTANTS_CRATES names crates' >&2; exit 2; }
[[ -z "$shard" || "$shard" =~ ^[0-9]+/[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_SHARD is K/N' >&2; exit 2; }
[[ -z "$jobs" || "$jobs" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_JOBS is a count' >&2; exit 2; }
[[ -z "$list" || ( "$list" =~ ^[A-Za-z0-9_./-]+$ && -s "$repo/$list" ) ]] ||
    { echo 'FXDAX_MUTANTS_LIST names a file in the repository' >&2; exit 2; }
purpose=fxdax-test
lifetime=8h limit=${FXDAX_GCE_LIMIT:-7h}
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
    gce_remote "sudo env FXDAX_MUTANTS_SHARD=$shard FXDAX_MUTANTS_JOBS=$jobs FXDAX_MUTANTS_LIST=$list \
        timeout --signal=TERM --kill-after=30s $limit \
        bash $run/source/scripts/lib/mutants-linux-host.sh $run/source $run/results $crates" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    tail -n 20 "$results/remote.log" >&2
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
