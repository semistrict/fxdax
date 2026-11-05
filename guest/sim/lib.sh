# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# What check.sh, mutants.sh and zloop.sh share, sourced with guest/sim as the directory: builds of
# the tests against a copy of the real sources, and the model of Fxfs, with patches in, each in
# $BUILD/mutants/<name>. Takes KERNEL and BUILD as the Makefile does.

# Every test, one a file of tests/: those that take a seed, those that play out what tests/scenarios
# names, and the rest, one run each, which a new file is unless it is listed here.
seeded=(interleave failures)
mapfile -t sim_tests < <(basename -s .c tests/*.c)
one_runs=()
for test in "${sim_tests[@]}"; do
    [[ " ${seeded[*]} scenarios " == *" $test "* ]] || one_runs+=("$test")
done
unset test

# Every run of one a build in `dir` has, a line each, the test and its arguments: the tests of one
# run, then each of what tests/scenarios plays out, by name.
sim_one_runs() { # dir
    local test
    printf '%s\n' "${one_runs[@]}"
    for test in $("$1/scenarios" 2>&1 | grep -v WARNING); do
        echo "scenarios $test"
    done
}

sim_prepare() {
    make -s -j"$(nproc)" "KERNEL=${KERNEL:?}" "BUILD=${BUILD:?}" compile || exit 1
    eval "$(make -s "KERNEL=$KERNEL" "BUILD=$BUILD" flags)"
    export ASAN_OPTIONS=detect_leaks=0:detect_stack_use_after_return=0
}

# The sources a build compiles, as `patch -p1` names them in `dir`, and where each is from.
sim_sources() { # dir
    echo "../linux/fs/fxfs/dax.c $1/guest/linux/fs/fxfs/dax.c"
    echo "../linux/fs/fxfs/pages.c $1/guest/linux/fs/fxfs/pages.c"
    echo "../linux/fs/fxfs/device.c $1/guest/linux/fs/fxfs/device.c"
    echo "fxfs/fxfs.c $1/guest/sim/fxfs/fxfs.c"
    echo "${KERNEL:?}/fs/dax.c $1/fs/dax.c"
}

# A copy of the sources in $BUILD/mutants/<name>, with the patches in. Each applies as it is, with no
# fuzz: one that needs any has drifted from the code it changes, and may change something else.
sim_tree() { # name, patches...
    local name=$1 dir="$BUILD/mutants/$1" from to patch
    shift
    while read -r from to; do
        mkdir -p "$(dirname "$to")"
        cp "$from" "$to"
    done < <(sim_sources "$dir")
    for patch in "$@"; do
        patch -s -F0 -p1 -d "$dir" < "$patch" > /dev/null ||
            { echo "$name: $patch doesn't apply" >&2; return 1; }
    done
}

# Whether the patches apply and reverse, leaving the sources as they were: that a run of them would
# test what they say.
sim_check() { # name, patches...
    local name=$1 dir="$BUILD/mutants/$1" from to i
    shift
    sim_tree "$name" "$@" || return 1
    for ((i = $#; i > 0; i--)); do
        patch -s -F0 -R -p1 -d "$dir" < "${!i}" > /dev/null ||
            { echo "$name: ${!i} doesn't reverse" >&2; return 1; }
    done
    while read -r from to; do
        cmp -s "$from" "$to" || { echo "$name: $to isn't as it was, reversed" >&2; return 1; }
    done < <(sim_sources "$dir")
}

sim_build() { # name, patches...
    local name=$1 dir="$BUILD/mutants/$1" test
    sim_tree "$@" || return 1
    # shellcheck disable=SC2086
    "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/dax.c" -o "$dir/dax.o" &&
        "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/pages.c" -o "$dir/pages.o" &&
        "$CC" $CFLAGS -c "$dir/guest/linux/fs/fxfs/device.c" -o "$dir/device.o" &&
        "$CC" $CFLAGS -c "$dir/guest/sim/fxfs/fxfs.c" -o "$dir/fxfs.o" &&
        "$CC" $CFLAGS -c "$dir/fs/dax.c" -o "$dir/fsdax.o" &&
        for test in "${sim_tests[@]}"; do
            "$CC" $LDFLAGS "$BUILD/tests/$test.o" "$dir/dax.o" "$dir/pages.o" "$dir/device.o" "$dir/fxfs.o" \
                "$dir/fsdax.o" ${OBJECTS//$BUILD\/fake\/fxfs\/fxfs.o/} -o "$dir/$test" || return 1
        done
}

# The line that says how a run's log failed, if it says.
sim_failure_line() { # log
    grep -m1 -E 'PANIC|DEADLOCK|refused|ERROR: Addr|SUMMARY' "$1"
}

# That line, short, or that the run was killed.
sim_failure() { # log
    local line
    line=$(sim_failure_line "$1" | cut -c1-120)
    echo "${line:-no failure logged: it ran out of time, or was killed}"
}

# The task of the open bug a run's log failed of, as known-failures lists them, if it is one's.
sim_known() { # log
    local line task pattern
    line=$(sim_failure_line "$1") || return 1
    while read -r task pattern; do
        [[ -z $task || $task == \#* ]] && continue
        if grep -qE -- "$pattern" <<< "$line"; then
            echo "$task"
            return 0
        fi
    done < known-failures
    return 1
}
