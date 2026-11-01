#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Puts each of mutants/*.patch into a copy of dax.c, device.c or fs/dax.c, builds the tests against
# it, and runs seeds until one fails: how many seeds a past bug takes to show, or that it doesn't.
#
#   mutants.sh [-b BASE.patch]... [-s SEEDS] [-o OPERATIONS] [NAME...]
#
# With -b, as many as asked, every build has BASE too: a fix a run needs out of its way. Takes
# KERNEL and BUILD as the Makefile does. dax.c itself stays as it is. A NAME with no patch builds
# what the -b fixes make, which is how to see they leave nothing failing.

set -u -o pipefail
cd "$(dirname "$0")"

bases=() seeds=2000 operations=200
while getopts b:s:o: opt; do
    case $opt in
        b) bases+=("$OPTARG") ;;
        s) seeds=$OPTARG ;;
        o) operations=$OPTARG ;;
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
        for test in interleave hole_mapped_then_written; do
            "$CC" $LDFLAGS "$BUILD/tests/$test.o" "$dir/dax.o" "$dir/device.o" "$dir/fsdax.o" \
                $OBJECTS -o "$dir/$test" || return 1
        done
}

printf '%-34s %-10s %-9s %s\n' mutant seeds time 'first failure'
for name in "${names[@]}"; do
    patches=("${bases[@]}")
    [[ -f mutants/$name.patch ]] && patches+=("mutants/$name.patch")
    build "$name" "${patches[@]}" || continue
    began=$(date +%s.%N)
    found="" seed=1
    while ((seed <= seeds)); do
        if ! timeout 120 "$BUILD/mutants/$name/interleave" "$seed" "$operations" \
            > "$BUILD/mutants/$name/log" 2>&1; then
            found=$(grep -E 'PANIC|DEADLOCK|refused|ERROR: Addr|SUMMARY' "$BUILD/mutants/$name/log" | head -1 | cut -c1-100)
            found=${found:-exit $?}
            break
        fi
        seed=$((seed + 1))
    done
    took=$(printf '%.1f' "$(echo "$(date +%s.%N) - $began" | bc)")
    if [[ -n $found ]]; then
        printf '%-34s %-10s %-9s %s\n' "$name" "$seed" "${took}s" "$found"
    else
        printf '%-34s %-10s %-9s %s\n' "$name" "none/$seeds" "${took}s" 'not caught'
    fi
done
