#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# What OpenZFS's zloop.sh is to ztest: every test here, then the seeds of each that takes them
# (interleave, failures) at once, all of them whatever fails, each failure's log kept with how to
# replay it. Exits 1 if anything failed but an open bug's failure, as known-failures lists them.
#
#   zloop.sh [-s SEEDS] [-f FIRST] [-o OPERATIONS] [-j JOBS] [-d LOGS] [-p PATCH]...
#
# Runs dax.c with fixes/*.patch in, until they are decided on: bugs already known aren't news. Those
# fixes stop applying once dax.c takes them, which fails this, to have them removed. -p puts a patch
# in too, as many as asked, such as a mutant, to see a hunt find it. Takes KERNEL and BUILD as the
# Makefile does; a seed in 8 at 1,000 operations, which reach what 200
# don't, unless -o says otherwise. LOGS/failures lists what failed, a line each, and LOGS/replay how
# to run each again.

set -u -o pipefail
cd "$(dirname "$0")"

first=1 seeds=3000 operations="" jobs=$(nproc) logs="" patches=()
while getopts s:f:o:j:d:p: opt; do
    case $opt in
        s) seeds=$OPTARG ;;
        f) first=$OPTARG ;;
        o) operations=$OPTARG ;;
        j) jobs=$OPTARG ;;
        d) logs=$OPTARG ;;
        p) patches+=("$OPTARG") ;;
        *) exit 2 ;;
    esac
done
. ./lib.sh
sim_prepare
logs=${logs:-$BUILD/zloop}
mkdir -p "$logs"
: > "$logs/failures"
fixes=()
shopt -s nullglob
for fix in fixes/*.patch; do
    fixes+=("$fix")
done
sim_build zloop "${fixes[@]}" "${patches[@]}" || exit 1
bin=$BUILD/mutants/zloop
{
    echo "# Each run that failed, again: build what it ran, with KERNEL and BUILD as it had them,"
    echo "# then each line runs one, as \$BUILD/mutants/zloop has it, with interleave's flags to see how."
    echo "guest/sim/zloop.sh -s 0${patches[*]/#/ -p }"
} > "$logs/replay"

# Says how a run failed, here and in LOGS/failures, with the open bug's task if it is one's; and
# how to run it again, in LOGS/replay.
failed() { # log, run...
    local log=$1 task said
    shift
    said="$*: $(sim_failure "$log")"
    task=$(sim_known "$log") && said="$said (known: $task)"
    echo "$said" | tee -a "$logs/failures"
    echo "\$BUILD/mutants/zloop/$*" >> "$logs/replay"
}

mapfile -t runs < <(sim_one_runs "$bin")
for one in "${runs[@]}"; do
    log=$logs/${one#scenarios }.log
    # shellcheck disable=SC2086
    timeout 120 "$bin/"$one > "$log" 2>&1 || failed "$log" $one
done

# A seed names a run: two runs of one must hash alike.
for test in "${seeded[@]}"; do
    one() { timeout 120 "$bin/$test" 1 2>&1 | grep -E 'passed|PANIC'; }
    if [[ $(one) != "$(one)" ]]; then
        echo "zloop: $test's seed 1 ran two ways: a run no longer replays" | tee -a "$logs/failures"
    fi
done

seed_run() { # seed
    local seed=$1 test log
    local count=${operations:-$((seed % 8 ? 200 : 1000))}
    for test in $seeded_tests; do
        log=$logs/$test-$seed.log
        if timeout 300 "$bin/$test" "$seed" "$count" > "$log" 2>&1; then
            rm -f "$log"
        else
            failed "$log" "$test" "$seed" "$count"
        fi
    done
}
export -f seed_run failed sim_failure sim_failure_line sim_known
export bin logs operations seeded_tests="${seeded[*]}"

began=$SECONDS
seq "$first" $((first + seeds - 1)) | xargs -P "$jobs" -I{} bash -c 'seed_run {}'
failed=$(wc -l < "$logs/failures")
counted=$(grep -vc '(known: ' "$logs/failures")
echo "zloop: $seeds seeds of ${seeded[*]} from $first: $failed runs failed, $((failed - counted)) of them known, in $((SECONDS - began)) s"
((failed)) && echo "zloop: logs in $logs; each run again as $logs/replay says, with interleave's flags to see how"
((counted == 0))
