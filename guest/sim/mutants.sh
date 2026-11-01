#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Puts each of mutants/*.patch into a copy of dax.c, device.c or fs/dax.c, builds the tests against
# it, and runs seeds until one fails: how many seeds a past bug takes to show, or that it doesn't.
#
#   mutants.sh [-b BASE.patch]... [-s SEEDS] [-o OPERATIONS] [-j JOBS] [-J SEED_JOBS] [NAME...]
#
# OPERATIONS, as zloop.sh has them, are 200, and 1,000 for a seed in 8. With -b, as many as asked,
# every build has BASE too: a fix a run needs out of its way. Takes
# KERNEL and BUILD as the Makefile does. dax.c itself stays as it is. A NAME with no patch builds
# what the -b fixes make, which is how to see they leave nothing failing. JOBS mutants run at once,
# each SEED_JOBS seeds at once.

set -u -o pipefail
cd "$(dirname "$0")"

bases=() seeds=2000 operations="" jobs=$(nproc) seed_jobs=1
while getopts b:s:o:j:J: opt; do
    case $opt in
        b) bases+=("$OPTARG") ;;
        s) seeds=$OPTARG ;;
        o) operations=$OPTARG ;;
        j) jobs=$OPTARG ;;
        J) seed_jobs=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))

. ./lib.sh
sim_prepare

names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    names=()
    for patch in mutants/*.patch; do
        names+=("$(basename "$patch" .patch)")
    done
fi

# Whether `$dir/$test` fails with these arguments: what it says, if so.
failed() { # dir, test, arguments...
    local dir=$1 test=$2
    shift 2
    timeout 120 "$dir/$test" "$@" > "$dir/log" 2>&1 && return 1
    sim_failure "$dir/log"
}

run() { # name
    local name=$1 dir=$BUILD/mutants/$1 patches=("${bases[@]}") found="" seed first test began took i stop=0
    [[ -f mutants/$name.patch ]] && patches+=("mutants/$name.patch")
    sim_build "$name" "${patches[@]}" || return
    for ((i = 0; i < seed_jobs; i++)); do
        mkdir -p "$dir/$i"
        cp "$dir/interleave" "$dir/$i/interleave"
    done
    began=$(date +%s.%N)
    for test in "${scenarios[@]}"; do
        if found=$(failed "$dir" "$test"); then
            found=${found:-$test}
            seed=$test
            break
        fi
    done
    if [[ -z $found ]]; then
        # Seeds at once, a batch at a time: the first of a batch to fail is the one to say.
        for ((seed = 1, first = 0; seed <= seeds && !stop; seed += seed_jobs)); do
            for ((i = 0; i < seed_jobs && seed + i <= seeds; i++)); do
                (failed "$dir/$i" interleave $((seed + i)) \
                    "${operations:-$(((seed + i) % 8 ? 200 : 1000))}" > "$dir/$i/failure") &
            done
            wait
            for ((i = 0; i < seed_jobs; i++)); do
                if [[ -s $dir/$i/failure ]]; then
                    found=$(cat "$dir/$i/failure")
                    first=$((seed + i))
                    stop=1
                    break
                fi
            done
        done
        seed=$first
    fi
    took=$(printf '%.1f' "$(echo "$(date +%s.%N) - $began" | bc)")
    if [[ -n $found ]]; then
        printf '%-34s %-24s %-9s %s\n' "$name" "$seed" "${took}s" "$found"
    else
        printf '%-34s %-24s %-9s %s\n' "$name" "none/$seeds" "${took}s" 'not caught'
    fi
}

printf '%-34s %-24s %-9s %s\n' mutant seeds time 'first failure'
for name in "${names[@]}"; do
    run "$name" &
    while (($(jobs -rp | wc -l) >= jobs)); do
        wait -n
    done
done
wait
