#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# How long a fork of a 10 GiB base takes, at commits of fxdax's, on an x86-64 Linux host of one's
# own, over ssh (FXDAX_HOST, ubu1 by default; scripts/lib/host.sh), as one measurement there, alone
# and on CPUs of its own (fxjob --bench) (TASK-165.7): a worktree of each COMMIT there, whose
# extent daemon and fxdaxctl this tree's host script (scripts/lib/fork-time-linux-host.sh) builds;
# it imports the base and times its forks and their first opens, for every commit on the one host.
# The results come back to RESULTS, each commit's record in RESULTS/LABEL/source.txt. A COMMIT
# need not have this script: an older commit measures as main's does.
#
#   scripts/fork-time-host.sh RESULTS LABEL=COMMIT...
#   scripts/fork-time-host.sh resume RUN RESULTS | cancel RUN
#
# Each COMMIT is one origin has. FXDAX_FORK_TIME_FILES, FXDAX_FORK_TIME_FILE_MIB and
# FXDAX_FORK_TIME_FORKS size the base and the forks (the host script), and reach the host, as
# every other FXDAX_* does (scripts/lib/host.sh). FXDAX_HOST_LIMIT caps the run (default 4h).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
limit=4h
host_bench=1

# finish STATUS says how the run ended, and returns STATUS.
finish() {
    local lines
    lines=$(wc -l < "$results/remote.log")
    sed -n "$((lines > 30 ? lines - 29 : 1)),\$p" "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$1"
}
host_after=finish
host_actions "$@"
(( $# >= 2 )) || { echo "Usage: $0 RESULTS LABEL=COMMIT... | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=$1
shift
pairs=("$@")
trees=()
for pair in "${pairs[@]}"; do
    trees+=("${pair%%=*}=$host_dir/trees/${pair%%=*}")
done

status=0
host_start "${pairs[@]}"
echo "Results: $results" >&2
host_job fork-time-linux-host.sh "${trees[@]}" > "$results/remote.log" 2>&1 || status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
