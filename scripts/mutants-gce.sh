#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing (scripts/mutants.sh, TASK-44.1) on a kept x86-64 GCE
# host, where fxdax's Linux-only code (the extent daemon, the DAX window's
# userfaultfd) builds and its tests run. Each crate's report comes back to the
# results directory (default: docs/measurements/gce-mutants-<time>-<mark>, the
# run's mark, scripts/lib/gce.sh).
#
#   scripts/mutants-gce.sh [all|create] [results-directory]
#   scripts/mutants-gce.sh run|stop|delete instance [results-directory]
#
# all acquires a kept host and stops it as it ends; create holds one for runs
# until its stop (gce_main).
#
# FXDAX_MUTANTS_CRATES names the crates, separated by spaces (default:
# scripts/mutants.sh's), FXDAX_MUTANTS_SHARD the shard (K/N) and
# FXDAX_MUTANTS_JOBS the mutants at once (default: a third of the host's CPUs);
# FXDAX_MUTANTS_LIST a file in the repository listing the mutants to run alone
# (scripts/mutants.sh).
# Shard a sweep longer than a run's limit (FXDAX_GCE_LIMIT,
# default 7h). FXDAX_GCE_PROJECT is the project (default: gcloud's),
# FXDAX_GCE_ZONE the zone to try first (default: us-east4-a) and
# FXDAX_MUTANTS_MACHINE the machine type (default: n2-standard-4). Every other
# FXDAX_* and PJDFSTEST_* variable set here reaches the host too
# (scripts/lib/gce.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_MUTANTS_MACHINE:-n2-standard-4}
crates=${FXDAX_MUTANTS_CRATES:-}
shard=${FXDAX_MUTANTS_SHARD:-}
jobs=${FXDAX_MUTANTS_JOBS:-}
list=${FXDAX_MUTANTS_LIST:-}
results=$repo/docs/measurements/gce-mutants-$(date -u +%Y-%m-%dT%H%MZ)-$gce_maker
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_MUTANTS_MACHINE is a machine type' >&2; exit 2; }
[[ "$crates" =~ ^[a-z_\ ]*$ ]] || { echo 'FXDAX_MUTANTS_CRATES names crates' >&2; exit 2; }
[[ -z "$shard" || "$shard" =~ ^[0-9]+/[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_SHARD is K/N' >&2; exit 2; }
[[ -z "$jobs" || "$jobs" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_JOBS is a count' >&2; exit 2; }
[[ -z "$list" || ( "$list" =~ ^[A-Za-z0-9_./-]+$ && -s "$repo/$list" ) ]] ||
    { echo 'FXDAX_MUTANTS_LIST names a file in the repository' >&2; exit 2; }
limit=${FXDAX_GCE_LIMIT:-7h}
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
    # shellcheck disable=SC2086 # The crates are names separated by spaces.
    gce_remote_detached "$run" "$(gce_host_command "$run" mutants-linux-host.sh $crates)" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    tail -n 20 "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$status"
}

gce_main "$@"
