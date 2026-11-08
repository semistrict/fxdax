#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# The sweep (TASK-149.15): each run of one, every test of one run and every scenario, run as it is
# to count the calls of the code under test that may fail (kernel/sweep.c), then again for each N
# of them with the Nth failing (SIM_SWEEP=N), once. What the test expects is void once it has; what
# holds whatever fails isn't: ASan, UBSan and LeakSanitizer, dax.c's WARNs, this kernel's checks, and
# the end of the run's (sim_end), the mount synced, its files read as Fxfs holds them, what they were
# told durable, unmounted. A run fails if it exits other than 0.
#
#   sweep.sh [-j JOBS] [-d LOGS] [-p PATCH]... [-s K/N] [RUN...]
#
# RUNs as sim_one_runs lists them (`scenarios cow_commit_failed`), every one by default, with
# read_pressure's of fewer readers (sim_sweep_runs). -s K/N sweeps a slice of each run's calls:
# every Nth, from the (K mod N)+1th, a sweep's Nth part in cost, every run in it, as CI's smoke run
# does, K its run's number, so that N runs of it go through every call (scripts/guest-sim-scope.py). Runs dax.c with fixes/*.patch in, as zloop.sh,
# and the -p patches, such as a mutant. Takes KERNEL and BUILD as the Makefile does. LOGS/failures
# lists what failed, a line each, and each run's log is kept; `SIM_SWEEP=N $BUILD/mutants/sweep/RUN`
# runs one again.

set -u -o pipefail
cd "$(dirname "$0")"

jobs=$(nproc) logs="" patches=() slice=""
while getopts j:d:p:s: opt; do
    case $opt in
        j) jobs=$OPTARG ;;
        d) logs=$OPTARG ;;
        p) patches+=("$OPTARG") ;;
        s) slice=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))
. ./lib.sh
sim_prepare
logs=${logs:-$BUILD/sweep}
mkdir -p "$logs"
: > "$logs/failures"
fixes=()
shopt -s nullglob
for fix in fixes/*.patch; do
    fixes+=("$fix")
done
sim_build sweep "${fixes[@]}" "${patches[@]}" || exit 1
bin=$BUILD/mutants/sweep
if (($#)); then
    runs=("$@")
else
    mapfile -t runs < <(sim_sweep_runs "$bin")
fi
if [[ -n $slice ]]; then
    [[ $slice =~ ^([0-9]+)/([1-9][0-9]*)$ ]] || { echo "sweep: -s K/N, not $slice" >&2; exit 2; }
    # Read by sim_sweep: the calls it fails, every sweep_every-th from sweep_from.
    sweep_every=${BASH_REMATCH[2]} sweep_from=$((BASH_REMATCH[1] % BASH_REMATCH[2] + 1))
    echo "sweep: every ${sweep_every}th call of each run, from call $sweep_from"
fi

began=$SECONDS total=0
for one in "${runs[@]}"; do
    # shellcheck disable=SC2086
    calls=$(sim_sweep_calls "$bin" $one) || { echo "$calls" | tee -a "$logs/failures"; continue; }
    total=$((total + (calls - ${sweep_from:-1} + ${sweep_every:-1}) / ${sweep_every:-1}))
    # shellcheck disable=SC2086
    sim_sweep "$bin" "$logs" "$jobs" $one | tee -a "$logs/failures"
    echo "sweep: $one: $calls calls"
done
failed=$(grep -c '' "$logs/failures")
echo "sweep: ${#runs[@]} runs, $total calls each failed in turn: $failed failed, in $((SECONDS - began)) s"
((failed)) && echo "sweep: logs in $logs"
((failed == 0))
