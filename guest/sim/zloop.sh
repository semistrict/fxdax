#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# What OpenZFS's zloop.sh is to ztest: every test here, then interleave's seeds at once, all of them
# whatever fails, each failure's log kept with how to replay it. Exits 1 if anything failed.
#
#   zloop.sh [-s SEEDS] [-f FIRST] [-o OPERATIONS] [-j JOBS] [-d LOGS] [-S]
#
# Runs dax.c with fixes/*.patch in, until they are decided on: bugs already known aren't news. Those
# fixes stop applying once dax.c takes them, which fails this, to have them removed. -S runs dax_shadow
# too. Takes KERNEL and BUILD as the Makefile does; a seed in 8 at 1,000 operations, which reach what
# 200 don't, unless -o says otherwise.

set -u -o pipefail
cd "$(dirname "$0")"

first=1 seeds=3000 operations="" jobs=$(nproc) logs="" shadow=""
while getopts s:f:o:j:d:S opt; do
    case $opt in
        s) seeds=$OPTARG ;;
        f) first=$OPTARG ;;
        o) operations=$OPTARG ;;
        j) jobs=$OPTARG ;;
        d) logs=$OPTARG ;;
        S) shadow=-s ;;
        *) exit 2 ;;
    esac
done
. ./lib.sh
sim_prepare
logs=${logs:-$BUILD/zloop}
mkdir -p "$logs"
fixes=()
shopt -s nullglob
for fix in fixes/*.patch; do
    fixes+=("$fix")
done
sim_build zloop "${fixes[@]}" || exit 1
bin=$BUILD/mutants/zloop
failures=0

for test in "${scenarios[@]}"; do
    if ! timeout 120 "$bin/$test" > "$logs/$test.log" 2>&1; then
        echo "zloop: $test failed: $(sim_failure "$logs/$test.log")"
        failures=$((failures + 1))
    fi
done

# A seed names a run: two runs of one must hash alike.
one() { timeout 120 "$bin/interleave" 1 2>&1 | grep -E 'passed|PANIC'; }
if [[ $(one) != "$(one)" ]]; then
    echo "zloop: seed 1 ran two ways: a run no longer replays"
    failures=$((failures + 1))
fi

seed_run() { # seed
    local seed=$1
    local count=${operations:-$((seed % 8 ? 200 : 1000))} log=$logs/seed-$1.log
    if timeout 300 "$bin/interleave" "$seed" "$count" $shadow > "$log" 2>&1; then
        rm -f "$log"
    else
        echo "seed $seed, $count operations: $(sim_failure "$log")"
    fi
}
export -f seed_run sim_failure
export bin logs operations shadow

began=$SECONDS
seq "$first" $((first + seeds - 1)) | xargs -P "$jobs" -I{} bash -c 'seed_run {}' | tee "$logs/failures"
failed=$(wc -l < "$logs/failures")
echo "zloop: $seeds seeds from $first, $failed failed, in $((SECONDS - began)) s"
((failed)) && echo "zloop: logs in $logs; replay with $bin/interleave SEED OPERATIONS, as a failure says"
((failures + failed == 0))
