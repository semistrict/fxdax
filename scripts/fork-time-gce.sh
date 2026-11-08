#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# How long a fork of a 10 GiB base takes, on trees of fxdax's, on one kept GCE host
# (TASK-165.7): acquires the host (scripts/lib/gce.sh), copies each TREE to it, and this tree,
# whose host script
# (scripts/lib/fork-time-linux-host.sh) builds each TREE's extent daemon and fxdaxctl, imports
# the base and times its forks and their first opens, on one host for all of them; fetches the
# results, and stops the host whatever happens. A TREE need not have this script: an older
# commit's, checked out with its Firecracker (git worktree add; git submodule update --init
# vendor/firecracker), measures as main's does.
#
#   scripts/fork-time-gce.sh RESULTS LABEL=TREE...
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's),
# FXDAX_GCE_ZONE the zone to try first (default: us-east4-a), FXDAX_GCE_MACHINE the machine type
# (default: n2-standard-4). FXDAX_FORK_TIME_FILES, FXDAX_FORK_TIME_FILE_MIB and
# FXDAX_FORK_TIME_FORKS size the base and the forks (the host script), and reach the host, as
# every other FXDAX_* does (scripts/lib/gce.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
usage() { echo "Usage: $0 RESULTS LABEL=TREE..." >&2; exit 2; }
(( $# >= 2 )) || usage
results=$1
shift
pairs=("$@")
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
for pair in "${pairs[@]}"; do
    [[ $pair =~ ^[a-z0-9-]+=. && -d ${pair#*=} ]] || { echo "$pair: LABEL=TREE, TREE a directory" >&2; exit 2; }
done
mkdir -p "$results"
results=$(cd "$results" && pwd)
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_GCE_MACHINE:-n2-standard-4}
limit=4h
create_flags=(--min-cpu-platform='Intel Cascade Lake')

# push_tree TREE RESULTS RUN copies TREE to RUN/source on the host, as gce_push_source copies this
# tree, recording it in RESULTS: gce.sh's repo and results are TREE's and RESULTS within.
push_tree() {
    local repo results=$2
    repo=$(cd "$1" && pwd) || return
    mkdir -p "$results" || return
    gce_push_source "$3"
}

measure() {
    local run status=0 pair label trees=()
    gce_check_owner || return
    gce_wait_ready || return
    run=$(gce_run_name)
    # Each tree in a directory of its own, run-...-LABEL/source, what it is recorded in
    # RESULTS/LABEL/source.txt.
    for pair in "${pairs[@]}"; do
        label=${pair%%=*}
        push_tree "${pair#*=}" "$results/$label" "$run-$label" || return
        trees+=("$label=$run-$label/source")
    done
    gce_push_source "$run" || return
    gce_remote_detached "$run" "$(gce_host_command "$run" fork-time-linux-host.sh "${trees[@]}")" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    tail -n 30 "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$status"
}

gce_all measure
