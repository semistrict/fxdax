# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# What mutants.sh and zloop.sh share, sourced with guest/sim as the directory: builds of the tests
# against a copy of the real sources, and the model of Fxfs, with patches in, each in
# $BUILD/mutants/<name>. Takes KERNEL and BUILD as the Makefile does.

# The tests of one run, which every build has, then interleave, its seeds.
scenarios=(writes_through read_pressure read_mappings_stay hole_mapped_then_written load_during_move pmd_over_stale_index open_extent_read_only
    sealed_piece_reach pmd_entry_invalidated_mapped)

# Every run of one a build in `dir` has, a line each, the test and its arguments: the tests of one
# run, then each of what tests/scenarios plays out, by name.
sim_one_runs() { # dir
    local test
    printf '%s\n' "${scenarios[@]}"
    for test in $("$1/scenarios" 2>&1 | grep -v WARNING); do
        echo "scenarios $test"
    done
}

sim_prepare() {
    make -s "KERNEL=${KERNEL:?}" "BUILD=${BUILD:?}" compile || exit 1
    eval "$(make -s "KERNEL=$KERNEL" "BUILD=$BUILD" flags)"
    export ASAN_OPTIONS=detect_leaks=0:detect_stack_use_after_return=0
}

sim_build() { # name, patches...
    local name=$1 dir="$BUILD/mutants/$1" patch test
    shift
    mkdir -p "$dir/guest/linux/fs/fxfs" "$dir/guest/sim/fxfs" "$dir/fs"
    cp ../linux/fs/fxfs/dax.c ../linux/fs/fxfs/device.c "$dir/guest/linux/fs/fxfs/"
    cp fxfs/fxfs.c "$dir/guest/sim/fxfs/"
    cp "$KERNEL/fs/dax.c" "$dir/fs/dax.c"
    for patch in "$@"; do
        patch -s -p1 -d "$dir" < "$patch" || { echo "$name: $patch doesn't apply" >&2; return 1; }
    done
    # shellcheck disable=SC2086
    "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/dax.c" -o "$dir/dax.o" &&
        "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/device.c" -o "$dir/device.o" &&
        "$CC" $CFLAGS -c "$dir/guest/sim/fxfs/fxfs.c" -o "$dir/fxfs.o" &&
        "$CC" $CFLAGS -c "$dir/fs/dax.c" -o "$dir/fsdax.o" &&
        for test in "${scenarios[@]}" interleave failures scenarios; do
            "$CC" $LDFLAGS "$BUILD/tests/$test.o" "$dir/dax.o" "$dir/device.o" "$dir/fxfs.o" \
                "$dir/fsdax.o" ${OBJECTS//$BUILD\/fake\/fxfs\/fxfs.o/} -o "$dir/$test" || return 1
        done
}

# The line that says how a run's log failed, or that the run was killed.
sim_failure() { # log
    local line
    line=$(grep -m1 -E 'PANIC|DEADLOCK|refused|ERROR: Addr|SUMMARY' "$1" | cut -c1-120)
    echo "${line:-no failure logged: it ran out of time, or was killed}"
}
