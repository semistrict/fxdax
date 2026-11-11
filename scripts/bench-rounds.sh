#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Rounds of guest test cases on this Linux host, a shared one (ubu1), each case run once a round,
# the cases' order reversed every other round (A B, B A, ...), so that what drifts on the host
# over the hours moves every case alike (TASK-350). Each run is a scripts/test-e2e.sh of the one
# case, as root (its cases attach loop devices), measured on cores of its own: `fxjob --bench`.
# Before each run it waits for the host's 1-minute load to fall below FXDAX_ROUNDS_MAX_LOAD
# (default 16) and drops the host's caches, the page cache and what of ZFS's ARC can go; and it
# records, in RESULTS/runs.tsv, each run's round, case, outcome, the load and the ARC's size
# before and after. A run whose load after passed the bound is marked `loaded`: its numbers are
# not to be reported, and the round to be run again.
#
#   scripts/bench-rounds.sh RESULTS ROUNDS CASE...
#
# Each run's results go to RESULTS/CASE/round-K: test-e2e.sh's logs, and the fs bench's report as
# JSON (FXDAX_BENCH_RESULTS), which scripts/bench-ab.py compares, a side's directories its cases'
# round directories. The environment's FXDAX_* variables pass to every run (sudo drops them
# otherwise): FXDAX_GUEST_KERNEL, FXDAX_BENCH_ITERATIONS, FXDAX_BENCH_GROUPS, FXDAX_PGBENCH_RUNS
# and such.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
(( $# >= 3 )) || { sed -n '/^#   scripts/p' "$0" >&2; exit 2; }
results=$1 rounds=$2
shift 2
cases=("$@")
[[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "ROUNDS is a count" >&2; exit 2; }
max_load=${FXDAX_ROUNDS_MAX_LOAD:-16}
mkdir -p "$results"
results=$(cd "$results" && pwd)
command -v fxjob > /dev/null || { echo "fxjob not found: this runs on a host with fxjob (ubu1)" >&2; exit 2; }

load() { cut -d' ' -f1 /proc/loadavg; }
arc_bytes() {
    awk '$1 == "size" { print $3 }' /proc/spl/kstat/zfs/arcstats 2> /dev/null || echo -
}

# Waits for the load to fall below the bound, saying so once a minute.
wait_for_quiet() {
    local waited=0
    while awk -v load="$(load)" -v max="$max_load" 'BEGIN { exit !(load >= max) }'; do
        (( waited % 60 == 0 )) && echo "waiting for the load ($(load)) to fall below $max_load" >&2
        sleep 10
        waited=$((waited + 10))
    done
}

# The FXDAX_* variables of this environment, as env's NAME=VALUE words.
passed=()
while IFS= read -r name; do
    passed+=("$name=${!name}")
done < <(compgen -e | grep '^FXDAX_' || true)

[[ -f $results/runs.tsv ]] ||
    printf 'round\tcase\toutcome\tload_before\tload_after\tarc_before\tarc_after\tstarted\n' \
        > "$results/runs.tsv"
status=0
for ((round = 1; round <= rounds; round++)); do
    order=("${cases[@]}")
    if (( round % 2 == 0 )); then
        order=()
        for ((i = ${#cases[@]} - 1; i >= 0; i--)); do order+=("${cases[i]}"); done
    fi
    for case in "${order[@]}"; do
        out=$results/$case/round-$round
        mkdir -p "$out"
        wait_for_quiet
        sync
        sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
        load_before=$(load) arc_before=$(arc_bytes) started=$(date -u +%FT%TZ)
        outcome=ok
        fxjob --bench --name "rounds-$case-$round" -- \
            sudo env "PATH=$PATH" "HOME=$HOME" "${passed[@]}" \
            "FXDAX_E2E_RESULTS=$out/e2e" "FXDAX_BENCH_RESULTS=$out" \
            "$repo/scripts/test-e2e.sh" "$case" > "$out/run.log" 2>&1 || outcome=failed
        load_after=$(load)
        if [[ $outcome == ok ]] &&
            awk -v load="$load_after" -v max="$max_load" 'BEGIN { exit !(load >= max) }'; then
            outcome=loaded
        fi
        [[ $outcome == ok ]] || status=1
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$round" "$case" "$outcome" "$load_before" \
            "$load_after" "$arc_before" "$(arc_bytes)" "$started" >> "$results/runs.tsv"
        echo "round $round $case: $outcome (load $load_before -> $load_after)" >&2
    done
done
sudo chown -R "$(id -u):$(id -g)" "$results"
exit "$status"
