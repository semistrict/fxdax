#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing (scripts/mutants.sh, TASK-44.1) on an x86-64 Linux host of one's own, over ssh
# (FXDAX_HOST, ubu1 by default; scripts/lib/host.sh), where fxdax's Linux-only code (the extent
# daemon, the DAX window's userfaultfd) builds and its tests run
# (scripts/lib/mutants-linux-host.sh). Each crate's report comes back to RESULTS (default:
# docs/measurements/HOST-mutants-<time>-<mark>, the run's mark).
#
#   scripts/mutants-host.sh [RESULTS]
#   scripts/mutants-host.sh resume RUN RESULTS | cancel RUN
#
# FXDAX_MUTANTS_CRATES names the crates, separated by spaces (default: scripts/mutants.sh's),
# FXDAX_MUTANTS_SHARD the shard (K/N) and FXDAX_MUTANTS_JOBS the mutants at once (default: a third
# of the host's CPUs); FXDAX_MUTANTS_LIST a file in the repository listing the mutants to run alone
# (scripts/mutants.sh). Shard a sweep longer than a run's limit (FXDAX_HOST_LIMIT, default 7h).
# Every other FXDAX_* and PJDFSTEST_* variable set here reaches the host too (scripts/lib/host.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
crates=${FXDAX_MUTANTS_CRATES:-}
shard=${FXDAX_MUTANTS_SHARD:-}
jobs=${FXDAX_MUTANTS_JOBS:-}
list=${FXDAX_MUTANTS_LIST:-}
[[ "$crates" =~ ^[a-z_\ ]*$ ]] || { echo 'FXDAX_MUTANTS_CRATES names crates' >&2; exit 2; }
[[ -z "$shard" || "$shard" =~ ^[0-9]+/[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_SHARD is K/N' >&2; exit 2; }
[[ -z "$jobs" || "$jobs" =~ ^[1-9][0-9]*$ ]] || { echo 'FXDAX_MUTANTS_JOBS is a count' >&2; exit 2; }
[[ -z "$list" || ( "$list" =~ ^[A-Za-z0-9_./-]+$ && -s "$repo/$list" ) ]] ||
    { echo 'FXDAX_MUTANTS_LIST names a file in the repository' >&2; exit 2; }
limit=7h

# finish STATUS says how the sweep ended, and returns STATUS.
finish() {
    local lines
    lines=$(wc -l < "$results/remote.log")
    sed -n "$((lines > 20 ? lines - 19 : 1)),\$p" "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$1"
}
host_after=finish
host_actions "$@"
(( $# <= 1 )) || { echo "Usage: $0 [RESULTS] | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=${1:-$repo/docs/measurements/$(host_label)-mutants-$(date -u +%Y-%m-%dT%H%MZ)-$host_mark}

status=0
host_start
echo "Results: $results" >&2
# shellcheck disable=SC2086 # The crates are names separated by spaces.
host_job mutants-linux-host.sh $crates > "$results/remote.log" 2>&1 || status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
