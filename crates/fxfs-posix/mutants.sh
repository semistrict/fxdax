#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each of mutants/*.patch into the tree, a bug of the kind the model test exists for, runs the
# crate's tests against it, and takes it out: which test finds it, how soon, or that none does.
#
#   mutants.sh [--check] [-c CASES] [NAME...]
#
# CASES is how many sequences the model test tries of each mutant: what it finds, it finds without
# shrinking. A mutant only that finds, and by chance, wants a test that always does: run this at the
# suite's own 96 to see. The tree has the mutant in it while it runs. Each patch applies as it is,
# with no fuzz: one that needs any has drifted from the code it changes. --check runs nothing: it
# applies and reverses each patch in a copy of what it changes, in a second, as CI does. A patch
# that doesn't apply fails either.

set -u -o pipefail
cd "$(dirname "$0")/../.."

check=""
if [[ ${1:-} == --check ]]; then
    check=1
    shift
fi
cases=96
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

# Whether a patch applies and reverses, leaving the files it changes as they were, in a copy of them.
round_trip() { # patch
    local copy file status=0
    copy=$(mktemp -d)
    for file in $(sed -n 's|^+++ b/||p' "$1"); do
        mkdir -p "$copy/$(dirname "$file")"
        cp "$file" "$copy/$file"
    done
    if ! patch -s -F0 -p1 -d "$copy" < "$1" > /dev/null; then
        echo "$1 doesn't apply" >&2
        status=1
    elif ! patch -s -F0 -R -p1 -d "$copy" < "$1" > /dev/null; then
        echo "$1 doesn't reverse" >&2
        status=1
    else
        for file in $(sed -n 's|^+++ b/||p' "$1"); do
            cmp -s "$file" "$copy/$file" || { echo "$1: $file isn't as it was, reversed" >&2; status=1; }
        done
    fi
    rm -r "$copy"
    return $status
}

broken=0
if [[ -n $check ]]; then
    for name in "${names[@]}"; do
        round_trip "$mutants/$name.patch" || broken=$((broken + 1))
    done
    echo "mutants: ${#names[@]} checked, $broken broken"
    exit $((broken > 0))
fi

printf '%-42s %-8s %s\n' mutant time 'found by'
for name in "${names[@]}"; do
    patch=$mutants/$name.patch
    if ! round_trip "$patch"; then
        printf '%-42s %-8s %s\n' "$name" - "BROKEN: its patch doesn't apply"
        broken=1
        continue
    fi
    patch -s -F0 -p1 < "$patch"
    log=$(mktemp)
    began=$SECONDS
    FXDAX_MODEL_CASES=$cases PROPTEST_MAX_SHRINK_ITERS=0 \
        cargo nextest run -p fxfs-posix > "$log" 2>&1
    status=$?
    patch -s -F0 -R -p1 < "$patch" || { echo "$name: its patch doesn't reverse: the tree has it" >&2; exit 1; }
    case $status in
        0) found='not caught' ;;
        100) found=$(grep -m1 -E '^ *(FAIL|SIGABRT|TIMEOUT) ' "$log" | sed 's/.*fxfs-posix //') ;;
        *) found="exit $status: $(grep -m1 -E '^error' "$log")" ;;
    esac
    printf '%-42s %-8s %s\n' "$name" "$((SECONDS - began))s" "$found"
    rm -f "$log"
done
exit $broken
