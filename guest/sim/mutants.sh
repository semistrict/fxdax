#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Puts each of mutants/*.patch into a copy of dax.c, device.c or fs/dax.c, builds the tests against
# it, and runs seeds until one fails: how many seeds a past bug takes to show, or that it doesn't.
#
#   mutants.sh [-b BASE.patch]... [-s SEEDS] [-o OPERATIONS] [-j JOBS] [NAME...]
#
# With -b, as many as asked, every build has BASE too: a fix a run needs out of its way. Takes
# KERNEL and BUILD as the Makefile does. dax.c itself stays as it is. A NAME with no patch builds
# what the -b fixes make, which is how to see they leave nothing failing. JOBS mutants run at once.

set -u -o pipefail
cd "$(dirname "$0")"

bases=() seeds=2000 operations=200 jobs=$(nproc)
while getopts b:s:o:j: opt; do
    case $opt in
        b) bases+=("$OPTARG") ;;
        s) seeds=$OPTARG ;;
        o) operations=$OPTARG ;;
        j) jobs=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))

make -s "KERNEL=${KERNEL:?}" "BUILD=${BUILD:?}" compile || exit 1
eval "$(make -s "KERNEL=$KERNEL" "BUILD=$BUILD" flags)"
export ASAN_OPTIONS=detect_leaks=0:detect_stack_use_after_return=0

names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    names=()
    for patch in mutants/*.patch; do
        names+=("$(basename "$patch" .patch)")
    done
fi

# Every test but smoke runs each build: scenarios first, as they take one run, then seeds.
tests=(hole_mapped_then_written load_during_move pmd_over_stale_index interleave)

build() { # name, patches...
    local name=$1 dir="$BUILD/mutants/$1" patch
    shift
    mkdir -p "$dir/guest/linux/fs/fxfs" "$dir/fs"
    cp ../linux/fs/fxfs/dax.c ../linux/fs/fxfs/device.c "$dir/guest/linux/fs/fxfs/"
    cp "$KERNEL/fs/dax.c" "$dir/fs/dax.c"
    for patch in "$@"; do
        patch -s -p1 -d "$dir" < "$patch" || { echo "$name: its patch doesn't apply" >&2; return 1; }
    done
    # shellcheck disable=SC2086
    "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/dax.c" -o "$dir/dax.o" &&
        "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/device.c" -o "$dir/device.o" &&
        "$CC" $CFLAGS -c "$dir/fs/dax.c" -o "$dir/fsdax.o" &&
        for test in "${tests[@]}"; do
            "$CC" $LDFLAGS "$BUILD/tests/$test.o" "$dir/dax.o" "$dir/device.o" "$dir/fsdax.o" \
                $OBJECTS -o "$dir/$test" || return 1
        done
}

# Whether `$dir/$test` fails with these arguments: what it says, if so.
failed() { # dir, test, arguments...
    local dir=$1 test=$2
    shift 2
    if ! timeout 120 "$dir/$test" "$@" > "$dir/log" 2>&1; then
        grep -E 'PANIC|DEADLOCK|refused|ERROR: Addr|SUMMARY' "$dir/log" | head -1 | cut -c1-100
        return 0
    fi
    return 1
}

run() { # name
    local name=$1 dir=$BUILD/mutants/$1 patches=("${bases[@]}") found="" seed test began took
    [[ -f mutants/$name.patch ]] && patches+=("mutants/$name.patch")
    build "$name" "${patches[@]}" || return
    began=$(date +%s.%N)
    for test in "${tests[@]}"; do
        [[ $test == interleave ]] && continue
        if found=$(failed "$dir" "$test"); then
            found=${found:-$test}
            seed=$test
            break
        fi
    done
    if [[ -z $found ]]; then
        for ((seed = 1; seed <= seeds; seed++)); do
            if found=$(failed "$dir" interleave "$seed" "$operations"); then
                found=${found:-exit}
                break
            fi
        done
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
