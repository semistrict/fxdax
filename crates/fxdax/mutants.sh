#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each of mutants/*.patch into the tree, a bug of a kind a compromised VM process's test
# exists to find (TASK-149.17: another tenant's reached, the daemon's memory past its bound, a path
# left held, commits wedged), runs the test its header names (`# Killer: TEST`, of fxdax's library)
# against it, and takes it out: killed if the test fails, MISSED if it passes, BROKEN if the patch
# doesn't apply or the tree doesn't build. This fails unless each is killed. Linux alone, as
# extentd is.
#
#   mutants.sh [--check] [-c CASES] [NAME...]
#
# CASES is how many cases the test tries of each mutant (FXDAX_HOSTILE_CASES), unshrunk. Each
# mutant's log is kept in $FXDAX_MUTANTS_LOGS (default: a new directory, which it names). The tree
# has the mutant in it while it runs. Each patch applies as it is, with no fuzz: one that needs any
# has drifted from the code it changes. --check runs nothing: it applies and reverses each patch in
# a copy of what it changes, in a second, as CI does. A patch that doesn't apply fails either.

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

mutants=crates/fxdax/mutants
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
        patch=$mutants/$name.patch
        round_trip "$patch" || broken=$((broken + 1))
        [[ -n $(sed -n 's/^# Killer: //p' "$patch") ]] || { echo "$patch names no killer" >&2; broken=$((broken + 1)); }
    done
    echo "mutants: ${#names[@]} checked, $broken broken"
    exit $((broken > 0))
fi

failed=0
logs=${FXDAX_MUTANTS_LOGS:-$(mktemp -d)}
mkdir -p "$logs"
echo "logs: $logs"
printf '%-40s %-8s %s\n' mutant time fate
for name in "${names[@]}"; do
    patch=$mutants/$name.patch
    killer=$(sed -n 's/^# Killer: //p' "$patch")
    if ! round_trip "$patch" || [[ -z $killer ]]; then
        printf '%-40s %-8s %s\n' "$name" - "BROKEN: its patch doesn't apply, or names no killer"
        failed=1
        continue
    fi
    patch -s -F0 -p1 < "$patch"
    log=$logs/$name.log
    began=$SECONDS
    FXDAX_HOSTILE_CASES=$cases PROPTEST_MAX_SHRINK_ITERS=0 \
        cargo nextest run -p fxdax --lib --no-fail-fast -E "test(=$killer)" > "$log" 2>&1
    status=$?
    patch -s -F0 -R -p1 < "$patch" || { echo "$name: its patch doesn't reverse: the tree has it" >&2; exit 1; }
    case $status in
        0) fate=MISSED; failed=1 ;;
        # What failed: proptest's reason, an allocation past the heap's bound, a signal, or an
        # assertion.
        100) fate="killed: $(sed -n 's/.*Fail(Reason("\([^"]*\).*/\1/p; s/.*\(bytes held, of at most [0-9]*\).*/\1/p; s/^ *ABORT \(SIG [0-9]*\) .*/\1/p; s/^ *\(assertion .*\)/\1/p' "$log" | head -1)" ;;
        *) fate="BROKEN: exit $status: $(sed -n '/^error/{p;q;}' "$log")"; failed=1 ;;
    esac
    printf '%-40s %-8s %s\n' "$name" "$((SECONDS - began))s" "${fate:0:160}"
done
exit $failed
