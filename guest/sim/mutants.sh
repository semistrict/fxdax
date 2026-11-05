#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Puts each of mutants/*.patch into a copy of dax.c, pages.c, device.c or fs/dax.c, builds the tests
# against it, and runs seeds until one fails: how many seeds a past bug takes to show, or that it doesn't.
#
#   mutants.sh [--check] [-b BASE.patch]... [-s SEEDS] [-o OPERATIONS] [-j JOBS] [-J SEED_JOBS] [NAME...]
#
# OPERATIONS, as zloop.sh has them, are 200, and 1,000 for a seed in 8. With -b, as many as asked,
# every build has BASE too: a fix a run needs out of its way. Takes
# KERNEL and BUILD as the Makefile does. dax.c itself stays as it is. A NAME with no patch builds
# what the -b fixes make, which is how to see they leave nothing failing. JOBS mutants run at once,
# each SEED_JOBS seeds at once. --check runs nothing: it applies and reverses each mutant's patches,
# with no fuzz, in seconds, as CI does. A mutant whose patch doesn't apply, here or in a run, fails
# this.

set -u -o pipefail
cd "$(dirname "$0")"

check=""
if [[ ${1:-} == --check ]]; then
    check=1
    shift
fi
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

names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    names=()
    for patch in mutants/*.patch; do
        names+=("$(basename "$patch" .patch)")
    done
fi

# A mutant's patches: the bases, then its own.
patches() { # name
    local patch
    for patch in "${bases[@]}"; do
        echo "$patch"
    done
    [[ ! -f mutants/$1.patch ]] || echo "mutants/$1.patch"
}

if [[ -n $check ]]; then
    broken=0
    for name in "${names[@]}"; do
        mapfile -t patches < <(patches "$name")
        sim_check "$name" "${patches[@]}" || broken=$((broken + 1))
    done
    echo "mutants: ${#names[@]} checked, $broken broken"
    ((broken == 0))
    exit
fi
sim_prepare

# Whether `$dir/$test` fails with these arguments: what it says, if so.
failed() { # dir, test, arguments...
    local dir=$1 test=$2
    shift 2
    timeout 120 "$dir/$test" "$@" > "$dir/log" 2>&1 && return 1
    sim_failure "$dir/log"
}

run() { # name
    local name=$1 dir=$BUILD/mutants/$1 patches found="" seed first one runs began took i stop=0
    mapfile -t patches < <(patches "$name")
    if ! sim_build "$name" "${patches[@]}"; then
        printf '%-34s %-24s %-9s %s\n' "$name" - - "BROKEN: it doesn't apply or build"
        echo "$name" >> "$BUILD/mutants/broken"
        return
    fi
    for ((i = 0; i < seed_jobs; i++)); do
        mkdir -p "$dir/$i"
        for test in "${seeded[@]}"; do
            cp "$dir/$test" "$dir/$i/"
        done
    done
    began=$(date +%s.%N)
    mapfile -t runs < <(sim_one_runs "$dir")
    for one in "${runs[@]}"; do
        # shellcheck disable=SC2086
        if found=$(failed "$dir" $one); then
            found=${found:-$one}
            seed=${one#scenarios }
            break
        fi
    done
    if [[ -z $found ]]; then
        # Seeds at once, a batch at a time: the first of a batch to fail is the one to say.
        for ((seed = 1, first = 0; seed <= seeds && !stop; seed += seed_jobs)); do
            for ((i = 0; i < seed_jobs && seed + i <= seeds; i++)); do
                (ops=${operations:-$(((seed + i) % 8 ? 200 : 1000))}
                 for test in "${seeded[@]}"; do
                     failed "$dir/$i" "$test" $((seed + i)) "$ops" && break
                 done) > "$dir/$i/failure" &
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

mkdir -p "$BUILD/mutants"
: > "$BUILD/mutants/broken"
printf '%-34s %-24s %-9s %s\n' mutant seeds time 'first failure'
for name in "${names[@]}"; do
    run "$name" &
    while (($(jobs -rp | wc -l) >= jobs)); do
        wait -n
    done
done
wait
[[ ! -s $BUILD/mutants/broken ]]
