#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Builds what's here and runs every scenario, then seeds of the tests that take them, in parallel,
# saying what failed:
#
#   check.sh [-s SEEDS] [-o OPERATIONS]
#
# Takes KERNEL and BUILD as the Makefile does.

set -u -o pipefail
cd "$(dirname "$0")"

seeds=200 operations=200
while getopts s:o: opt; do
    case $opt in
        s) seeds=$OPTARG ;;
        o) operations=$OPTARG ;;
        *) exit 2 ;;
    esac
done

make -s -j"$(nproc)" "KERNEL=${KERNEL:?}" "BUILD=${BUILD:?}" compile || exit 1
export ASAN_OPTIONS=detect_leaks=0:detect_stack_use_after_return=0
tests=$BUILD/tests log=$BUILD/check
mkdir -p "$log"

run() { # name, command...
    local name=$1
    shift
    if ! timeout 300 "$@" > "$log/$name.log" 2>&1; then
        printf '%s: %s\n' "$name" \
            "$(grep -E 'PANIC|DEADLOCK|refused|ERROR: Addr|SUMMARY' "$log/$name.log" | head -1 | cut -c1-140)"
        return 1
    fi
}
export -f run
export tests log

for test in smoke hole_mapped_then_written; do
    run "$test" "$tests/$test" || failed=1
done
names=$("$tests/scenarios" 2>&1 | grep -v WARNING)
failures=$(
    {
        for name in $names; do echo "$name $tests/scenarios $name"; done
        for seed in $(seq 1 "$seeds"); do
            echo "interleave-$seed $tests/interleave $seed $operations"
            echo "failures-$seed $tests/failures $seed $operations"
        done
    } | xargs -P "$(nproc)" -L 1 bash -c 'run "$@"' _
)
printf '%s' "$failures" | sort -V
echo
printf '%d failed of %d scenarios and %d seeds\n' \
    "$(printf '%s' "$failures" | grep -c '')" "$(wc -w <<< "$names")" "$seeds"
[[ -z $failures && -z ${failed:-} ]]
