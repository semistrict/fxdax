#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each of mutants/*.patch into the tree, a bug of a kind a compromised VM process's test
# exists to find (TASK-149.17: another tenant's reached, the daemon's memory past its bound, a path
# left held, commits wedged), or the resource budgets' and the degraded bucket's soak (TASK-330:
# bytes written twice, memory held twice, failures unsaid), runs the test its header names
# (`# Killer: TEST`, of fxdax's library, ignored or not: the soak is) against it, and takes it out:
# killed if the test fails, MISSED if it passes, BROKEN if the patch doesn't apply or the tree
# doesn't build. A killer may be a script of the repository's instead, with its arguments
# (`# Killer: scripts/sandboxes.sh short`, guests on KVM, TASK-330.1), which builds what it runs:
# killed if it exits 1, a run that failed, MISSED if 0, BROKEN otherwise, a run it couldn't set
# up; its fate says the checks it says failed. This fails unless each is killed. Linux alone, as
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

# patch_round_trip (scripts/lib/patches.sh): whether a patch is well formed, applies and reverses.
source scripts/lib/patches.sh

broken=0
if [[ -n $check ]]; then
    for name in "${names[@]}"; do
        patch=$mutants/$name.patch
        patch_round_trip "$patch" || broken=$((broken + 1))
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
    if ! patch_round_trip "$patch" || [[ -z $killer ]]; then
        printf '%-40s %-8s %s\n' "$name" - "BROKEN: its patch doesn't apply, or names no killer"
        failed=1
        continue
    fi
    patch -s -F0 -p1 < "$patch"
    log=$logs/$name.log
    began=$SECONDS
    if [[ $killer == scripts/* ]]; then
        read -ra script <<< "$killer"
        "${script[@]}" > "$log" 2>&1
        status=$?
        # A run that failed exits 1, as a test that failed exits 100.
        case $status in 1) status=100 ;; 100) status=2 ;; esac
    else
        FXDAX_HOSTILE_CASES=$cases PROPTEST_MAX_SHRINK_ITERS=0 \
            cargo nextest run -p fxdax --lib --no-fail-fast --run-ignored all -E "test(=$killer)" \
            > "$log" 2>&1
        status=$?
    fi
    patch -s -F0 -R -p1 < "$patch" || { echo "$name: its patch doesn't reverse: the tree has it" >&2; exit 1; }
    case $status in
        0) fate=MISSED; failed=1 ;;
        # What failed: proptest's reason, an allocation past the heap's bound, a signal, an
        # assertion, or a run's check.
        100) fate="killed: $(sed -n 's/.*Fail(Reason("\([^"]*\).*/\1/p; s/.*\(bytes held, of at most [0-9]*\).*/\1/p; s/^ *ABORT \(SIG [0-9]*\) .*/\1/p; s/^ *\(assertion .*\)/\1/p; /panicked at/{n;s/^ *//p;}; s/.*check failed: //p' "$log" | head -1)" ;;
        *) fate="BROKEN: exit $status: $(sed -n '/^error/{p;q;}' "$log")"; failed=1 ;;
    esac
    printf '%-40s %-8s %s\n' "$name" "$((SECONDS - began))s" "${fate:0:160}"
done
exit $failed
