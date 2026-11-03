#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Builds what's here and runs every test of one run and every scenario, then seeds of the tests
# that take them, in parallel, saying what failed:
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

. ./lib.sh
sim_prepare
tests=$BUILD/tests log=$BUILD/check
mkdir -p "$log"

run() { # name, command...
    local name=$1
    shift
    if ! timeout 300 "$@" > "$log/$name.log" 2>&1; then
        printf '%s: %s\n' "$name" "$(sim_failure "$log/$name.log")"
        return 1
    fi
}
export -f run sim_failure sim_failure_line
export tests log

mapfile -t runs < <(sim_one_runs "$tests")
failures=$(
    {
        for one in "${runs[@]}"; do echo "${one#scenarios } $tests/$one"; done
        for seed in $(seq 1 "$seeds"); do
            for test in "${seeded[@]}"; do
                echo "$test-$seed $tests/$test $seed $operations"
            done
        done
    } | xargs -P "$(nproc)" -L 1 bash -c 'run "$@"' _
)
printf '%s' "$failures" | sort -V
echo
printf '%d failed of %d runs of one and %d seeds of %s\n' \
    "$(printf '%s' "$failures" | grep -c '')" "${#runs[@]}" "$seeds" "${seeded[*]}"
[[ -z $failures ]]
