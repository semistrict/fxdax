#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each of mutants/*.patch into the tree, a bug of a kind the simulation of extent_remote's
# protocol exists to find (TASK-149.32: an object an acknowledged segment names gone, an upload never
# woken), runs the simulation's test its header names (`# Killer: TEST`, extent_store's, TASK-88,
# built with the profile sim, as scripts/test.sh's sim part builds it) against it, and takes it out: killed if the test fails, MISSED if it passes,
# BROKEN if the patch doesn't apply or the tree doesn't build. This fails unless each is killed.
#
#   mutants.sh [--check] [NAME...]
#
# Each mutant's log is kept in $FXDAX_MUTANTS_LOGS (default: a new directory, which it names). The
# tree has the mutant in it while it runs. Each patch applies as it is, with no fuzz: one that needs
# any has drifted from the code it changes. --check runs nothing: it applies and reverses each patch
# in a copy of what it changes, in a second, as CI does. A patch that doesn't apply fails either.

set -u -o pipefail
cd "$(dirname "$0")/../.."

check=""
if [[ ${1:-} == --check ]]; then
    check=1
    shift
fi

mutants=crates/extent_remote/mutants
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
    cargo nextest run --cargo-profile sim -p extent_store --no-fail-fast -E "test(=$killer)" \
        > "$log" 2>&1
    status=$?
    patch -s -F0 -R -p1 < "$patch" || { echo "$name: its patch doesn't reverse: the tree has it" >&2; exit 1; }
    case $status in
        0) fate=MISSED; failed=1 ;;
        # What failed: the simulation's first violation of the seed that found it.
        100) fate="killed: $(sed -n 's/^ *seed \([0-9]*\): \[$/seed \1/p; s/^ *"\(.*\)",\{0,1\}$/\1/p' "$log" | head -2 | paste -sd ' ' -)" ;;
        *) fate="BROKEN: exit $status: $(sed -n '/^error/{p;q;}' "$log")"; failed=1 ;;
    esac
    printf '%-40s %-8s %s\n' "$name" "$((SECONDS - began))s" "${fate:0:160}"
done
exit $failed
