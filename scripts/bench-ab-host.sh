#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# An A/B of the fs bench on an x86-64 Linux host of one's own, over ssh (FXDAX_HOST, ubu1 by
# default; scripts/lib/host.sh), as one measurement there, alone and on CPUs of its own (fxjob
# --bench; scripts/lib/bench-ab-linux-host.sh, TASK-276): each side's commit built there once, in a
# worktree of its own, into artifacts of its own, then ROUNDS rounds of both sides interleaved, A B,
# then B A, against those artifacts, so that what drifts on the host over the hours moves both
# alike and no round rebuilds anything; then scripts/bench-ab.py's comparison, the ext4 case the
# control, into RESULTS/ab.md.
#
#   scripts/bench-ab-host.sh [RESULTS]
#   scripts/bench-ab-host.sh resume RUN RESULTS | cancel RUN
#
# The sides: FXDAX_AB_A and FXDAX_AB_B, NAME=COMMIT each, a commit origin has; FXDAX_AB_B's COMMIT
# may be `same`, A's, built once, the sides told apart by FXDAX_AB_A_ENV and FXDAX_AB_B_ENV alone,
# NAME=VALUE words each side's rounds run with (such as
# FXDAX_GUEST_CMDLINE=virtio_extent.affine=0). FXDAX_AB_ROUNDS is the rounds (default 3),
# FXDAX_BENCH_ITERATIONS each round's iterations (default 2), FXDAX_AB_CONTROL the control's case
# (default ext4-guest-kernel; empty: none), FXDAX_AB_BUILD_NICE the niceness the sides build at
# (default 19). The cases, groups and vCPUs are bench-host.sh's (FXDAX_BENCH_CASES,
# FXDAX_BENCH_GROUPS, FXDAX_BENCH_VCPUS), the same for both sides. The scripts that run are those of
# the commit this tree's scripts run (scripts/lib/host.sh's FXDAX_HOST_COMMIT), the sides' commits'
# code what they build and boot. The results go to docs/measurements/HOST-fs-bench-ab-<time>-<mark>
# unless named; each side's record in RESULTS/NAME/source.txt.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
limit=8h
host_bench=1
export FXDAX_AB_BUILD_NICE=${FXDAX_AB_BUILD_NICE:-19}

# The name and commit of side SPEC, NAME=COMMIT, as `name commit` on one line.
side() {
    local spec=$1
    [[ $spec =~ ^([a-z0-9][a-z0-9-]*)=(.+)$ ]] ||
        { echo "a side is NAME=COMMIT, NAME in lower-case letters, digits and -: $spec" >&2; return 2; }
    echo "${BASH_REMATCH[1]} ${BASH_REMATCH[2]}"
}

# finish STATUS shows the comparison, and returns STATUS.
finish() {
    if [[ -f $results/ab.md ]]; then cat "$results/ab.md" >&2; fi
    echo "Results: $results" >&2
    return "$1"
}
host_after=finish
host_actions "$@"
(( $# <= 1 )) || { echo "Usage: $0 [RESULTS] | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=${1:-$repo/docs/measurements/$(host_label)-fs-bench-ab-$(date -u +%Y-%m-%dT%H%MZ)-$host_mark}

a=$(side "${FXDAX_AB_A:?FXDAX_AB_A names side A, NAME=COMMIT}")
b=$(side "${FXDAX_AB_B:?FXDAX_AB_B names side B, NAME=COMMIT}")
read -r a_name a_commit <<< "$a"
read -r b_name b_commit <<< "$b"
[[ $a_name != "$b_name" ]] || { echo "the sides need names of their own" >&2; exit 2; }
rounds=${FXDAX_AB_ROUNDS:-3}
[[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_AB_ROUNDS is a count" >&2; exit 2; }
sides=("$a_name=$a_commit")
b_source=same
if [[ $b_commit != same ]]; then
    sides+=("$b_name=$b_commit")
    b_source=$host_dir/trees/$b_name
fi

status=0
host_start "${sides[@]}"
echo "Results: $results" >&2
host_job bench-ab-linux-host.sh "$host_dir/trees/$a_name" "$b_source" "$rounds" "$a_name" \
    "$b_name" > "$results/remote.log" 2>&1 || status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
