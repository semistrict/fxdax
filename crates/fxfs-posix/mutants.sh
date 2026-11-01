#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each of mutants/*.patch into the tree, a bug of the kind the model test exists for, runs the
# model tests against it, and takes it out: which test finds it, how soon, or that none does.
#
#   mutants.sh [-c CASES] [NAME...]
#
# CASES is how many sequences the model test tries of each mutant: what it finds, it finds without
# shrinking. The tree has the mutant in it while it runs.

set -u -o pipefail
cd "$(dirname "$0")/../.."

cases=500
while getopts c: opt; do
    case $opt in
        c) cases=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))

mutants=crates/fxfs-posix/mutants
names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    for patch in "$mutants"/*.patch; do
        names+=("$(basename "$patch" .patch)")
    done
fi

printf '%-42s %-8s %s\n' mutant time 'found by'
for name in "${names[@]}"; do
    patch=$mutants/$name.patch
    patch -s -p1 < "$patch" || { echo "$name: its patch doesn't apply" >&2; continue; }
    log=$(mktemp)
    began=$SECONDS
    FXDAX_MODEL_CASES=$cases PROPTEST_MAX_SHRINK_ITERS=0 \
        cargo nextest run -p fxfs-posix model_tests > "$log" 2>&1
    status=$?
    patch -s -R -p1 < "$patch"
    case $status in
        0) found='not caught' ;;
        100) found=$(grep -m1 -E '^ *(FAIL|SIGABRT|TIMEOUT) ' "$log" | sed 's/.*fxfs-posix //') ;;
        *) found="exit $status: $(grep -m1 -E '^error' "$log")" ;;
    esac
    printf '%-42s %-8s %s\n' "$name" "$((SECONDS - began))s" "$found"
    rm -f "$log"
done
