#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# An A/B of the fs bench on a kept x86-64 GCE host (scripts/lib/gce.sh), as one run on it
# (scripts/lib/bench-ab-linux-host.sh, TASK-276): each side's tree built there once into artifacts
# of its own, then ROUNDS rounds of both sides interleaved, A B, then B A, against those artifacts,
# so that what drifts on the host over the hours moves both alike and no round rebuilds anything;
# then scripts/bench-ab.py's comparison, the ext4 case the control, into RESULTS/ab.md.
#
#   scripts/bench-ab-gce.sh all|create [RESULTS]
#   scripts/bench-ab-gce.sh run|stop|delete INSTANCE [RESULTS]
#
# run's sides: FXDAX_AB_A and FXDAX_AB_B, NAME=TREE each, a tree a checkout (a worktree, its
# vendor/firecracker initialized), no one edits while it ships; FXDAX_AB_B's TREE may be `same`,
# A's, built once, the sides told apart by FXDAX_AB_A_ENV and FXDAX_AB_B_ENV alone, NAME=VALUE
# words each side's rounds run with (such as FXDAX_GUEST_CMDLINE=virtio_extent.affine=0).
# FXDAX_AB_ROUNDS is the rounds (default 3), FXDAX_BENCH_ITERATIONS each round's iterations
# (default 2), FXDAX_AB_CONTROL the control's case (default ext4-guest-kernel; empty: none). The
# cases, groups and vCPUs are bench-gce.sh's (FXDAX_BENCH_CASES, FXDAX_BENCH_GROUPS,
# FXDAX_BENCH_VCPUS), the same for both sides. The scripts that run are this tree's, the sides'
# trees' code what they build and boot. FXDAX_BENCH_MACHINE, FXDAX_BENCH_CPU_PLATFORM and
# FXDAX_GCE_ZONE choose the host as bench-gce.sh's do.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
machine=${FXDAX_BENCH_MACHINE:-c3-standard-8}
results=$repo/docs/measurements/gce-fs-bench-ab-$(date -u +%Y-%m-%dT%H%MZ)-$gce_maker
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$machine" =~ ^[a-z0-9]+-[a-z]+-[0-9]+$ ]] || { echo 'FXDAX_BENCH_MACHINE is a machine type' >&2; exit 2; }
limit=${FXDAX_GCE_LIMIT:-8h}
create_flags=()
if [[ -n ${FXDAX_BENCH_CPU_PLATFORM:-} ]]; then
    create_flags=(--min-cpu-platform="$FXDAX_BENCH_CPU_PLATFORM")
fi

# The name and tree of side SPEC, NAME=TREE, as `name tree` on one line.
side() {
    local spec=$1
    [[ $spec =~ ^([a-z0-9][a-z0-9-]*)=(.+)$ ]] ||
        { echo "a side is NAME=TREE, NAME in lower-case letters, digits and -: $spec" >&2; return 2; }
    echo "${BASH_REMATCH[1]} ${BASH_REMATCH[2]}"
}

run() {
    local a_name a_tree b_name b_tree tools=$repo out run status=0 b_source=same
    read -r a_name a_tree <<< "$(side "${FXDAX_AB_A:?FXDAX_AB_A names side A, NAME=TREE}")"
    read -r b_name b_tree <<< "$(side "${FXDAX_AB_B:?FXDAX_AB_B names side B, NAME=TREE}")"
    [[ $a_name != "$b_name" ]] || { echo "the sides need names of their own" >&2; return 2; }
    [[ -d $a_tree ]] || { echo "$a_tree: no tree" >&2; return 2; }
    [[ $b_tree == same || -d $b_tree ]] || { echo "$b_tree: no tree" >&2; return 2; }
    local rounds=${FXDAX_AB_ROUNDS:-3}
    [[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_AB_ROUNDS is a count" >&2; return 2; }
    gce_check_owner
    mkdir -p "$results"
    out=$(cd "$results" && pwd)
    echo "Results: $out" >&2
    gce_wait_ready
    run=$(gce_run_name)
    # This tree's scripts, then each side's tree, each shipped as gce_push_source ships one, its
    # record in a directory of its own.
    mkdir -p "$out/tools" "$out/$a_name"
    results=$out/tools repo=$tools gce_push_source "$run"
    results=$out/$a_name repo=$(cd "$a_tree" && pwd) gce_push_source "$run/a"
    if [[ $b_tree != same ]]; then
        mkdir -p "$out/$b_name"
        results=$out/$b_name repo=$(cd "$b_tree" && pwd) gce_push_source "$run/b"
        b_source=$run/b/source
    fi
    results=$out
    gce_remote_detached "$run" "$(gce_host_command "$run" bench-ab-linux-host.sh \
        "$run/a/source" "$b_source" "$rounds" "$a_name" "$b_name")" \
        > "$out/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    [[ ! -f $out/ab.md ]] || cat "$out/ab.md" >&2
    echo "Results: $out" >&2
    return "$status"
}

gce_main "$@"
