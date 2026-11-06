#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Puts each past bug of ../mutants whose header names this simulator as its kernel (`Kernel: sim`,
# ../mutants/README.md) into a copy of dax.c, pages.c, data.c, device.c, the model of Fxfs or
# fs/dax.c, builds the tests against it, and runs its killer, the run its header names
# (`Killer:`): a test of one run, a scenario (`scenarios NAME`), a test that takes seeds
# (`interleave`, `failures`), run seed after seed until one fails, or a sweep of a run of one
# (`sweep scenarios NAME`, sweep.sh), each of its calls failing in turn. The mutant is killed only if its
# killer fails; MISSED if it passes; BROKEN if it doesn't apply or build, or its killer is no run
# here. One whose killer is `none`, which nothing here can kill, its header saying why, isn't run.
#
#   mutants.sh [--check] [-b BASE.patch]... [-s SEEDS] [-o OPERATIONS] [-j JOBS] [-J SEED_JOBS] [NAME...]
#
# NAMEs are mutants, every one of the simulator's by default. SEEDS, 2,000 by default, are as
# many as a seeded killer has; OPERATIONS, as zloop.sh has them, 200, and 1,000 for a seed in 8.
# With -b, as many as asked, every build has BASE too: a fix a run needs out of its way. Takes
# KERNEL and BUILD as the Makefile does (linux.sh, elsewhere than on Linux). dax.c itself stays as
# it is. JOBS mutants run at once, each SEED_JOBS seeds at once. Exits 1 unless each mutant run
# was killed. --check runs nothing: it applies and reverses each mutant's patches, with no fuzz, in
# seconds, as CI does, and fails if one doesn't, or names no killer.

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

# A field of a mutant's header: `# FIELD: value`.
field() { # name, field
    sed -n "s/^# $2: //p" "../mutants/$1.patch"
}

names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    for patch in ../mutants/*.patch; do
        name=$(basename "$patch" .patch)
        [[ $(field "$name" Kernel) != sim ]] || names+=("$name")
    done
fi

# A mutant's patches: the bases, then its own.
patches() { # name
    printf '%s\n' "${bases[@]}" "../mutants/$1.patch"
}

if [[ -n $check ]]; then
    broken=0
    for name in "${names[@]}"; do
        mapfile -t patches < <(patches "$name")
        if [[ -z $(field "$name" Killer) ]]; then
            echo "$name: its header names no killer" >&2
            broken=$((broken + 1))
        elif ! sim_check "$name" "${patches[@]}"; then
            broken=$((broken + 1))
        fi
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

# A mutant's fate, a line: its fate, name, killer, and what the killer said, or why it has none.
fate() { # fate, name, killer, said
    printf '%-8s %-44s %-40s %s\n' "$@"
    [[ $1 == killed || $1 == none ]] || echo "$2" >> "$BUILD/mutants/survived"
}

# Its killer's seeds, SEED_JOBS at once, until one fails: the first of a batch to fail is the one
# to say, as `seed N: what it said`.
seeded() { # dir, test
    local dir=$1 test=$2 seed i
    for ((i = 0; i < seed_jobs; i++)); do
        mkdir -p "$dir/$i"
        cp "$dir/$test" "$dir/$i/"
    done
    for ((seed = 1; seed <= seeds; seed += seed_jobs)); do
        for ((i = 0; i < seed_jobs && seed + i <= seeds; i++)); do
            (failed "$dir/$i" "$test" $((seed + i)) "${operations:-$(((seed + i) % 8 ? 200 : 1000))}" ||
                true) > "$dir/$i/failure" &
        done
        wait
        for ((i = 0; i < seed_jobs; i++)); do
            if [[ -s $dir/$i/failure ]]; then
                echo "seed $((seed + i)): $(cat "$dir/$i/failure")"
                return
            fi
        done
    done
    return 1
}

run() { # name
    local name=$1 dir=$BUILD/mutants/$1 killer patches said began
    killer=$(field "$name" Killer)
    if [[ $killer == none ]]; then
        fate none "$name" - "nothing here kills it: see its header"
        return
    fi
    mapfile -t patches < <(patches "$name")
    mkdir -p "$dir"
    if ! sim_build "$name" "${patches[@]}" > "$dir/build.log" 2>&1; then
        fate BROKEN "$name" "$killer" "it doesn't apply or build: $dir/build.log"
        return
    fi
    began=$SECONDS
    if [[ $killer == "sweep "* ]] && sim_sweep_runs "$dir" | grep -xF -- "${killer#sweep }" > /dev/null; then
        # shellcheck disable=SC2086
        said=$(sim_sweep "$dir" "$dir/sweep" "$seed_jobs" ${killer#sweep } | head -1)
    elif [[ " ${seeded[*]} " == *" $killer "* ]]; then
        said=$(seeded "$dir" "$killer") || said=""
    elif sim_one_runs "$dir" | grep -xF -- "$killer" > /dev/null; then
        # shellcheck disable=SC2086
        said=$(failed "$dir" $killer) || said=""
    else
        fate BROKEN "$name" "$killer" "its killer is no run here (sim_one_runs, sweep of one, or ${seeded[*]})"
        return
    fi
    if [[ -n $said ]]; then
        fate killed "$name" "$killer" "$((SECONDS - began)) s: $said"
    else
        fate MISSED "$name" "$killer" "$((SECONDS - began)) s: it passed"
    fi
}

mkdir -p "$BUILD/mutants"
: > "$BUILD/mutants/survived"
printf '%-8s %-44s %-40s %s\n' fate mutant killer 'what it said'
for name in "${names[@]}"; do
    run "$name" &
    while (($(jobs -rp | wc -l) >= jobs)); do
        wait -n
    done
done
wait
[[ ! -s $BUILD/mutants/survived ]]
