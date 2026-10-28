#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The gate a change to the guest's DAX code passes before it is pushed (TASK-119.8): on two GCE
# test hosts (scripts/test-gce.sh create), HEAD's DAX suites, rounds of each, the checks with
# delays injected (fxfs-guest-checks-dax-delay), the crash test, and pgbench-dax, whose data is
# checked; and every hand mutant of dax.c (scripts/guest-mutants-gce.py) killed by the suites
# within FXDAX_GATE_MUTANT_ROUNDS rounds (default 3). A pass is recorded as a note on HEAD
# (`git notes --ref=dax-gate`), which scripts/hooks/pre-push asks for of a push changing the
# guest's DAX code: `git config core.hooksPath scripts/hooks` installs it.
#
# Usage: dax-gate.sh HOST_A HOST_B [ZONE] — the hosts' zone (default: us-east4-a).
# FXDAX_GATE_ROUNDS is the suites' rounds (default 5). Results go to /tmp/fxdax-gate-<commit>.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
(( $# >= 2 )) || { sed -n '6,15p' "$0" >&2; exit 2; }
host_a=$1 host_b=$2 zone=${3:-us-east4-a}
rounds=${FXDAX_GATE_ROUNDS:-5}
mutant_rounds=${FXDAX_GATE_MUTANT_ROUNDS:-3}

cd "$root"
if ! git diff --quiet HEAD --; then
    echo "dax-gate: the tree differs from HEAD: commit first, the gate passes a commit" >&2
    exit 1
fi
commit=$(git rev-parse HEAD)
results=/tmp/fxdax-gate-${commit:0:12}
mkdir -p "$results"
echo "dax-gate: $commit, results in $results"

export FXDAX_GCE_ZONE=$zone FXDAX_WORKSPACE_TESTS=0 FXDAX_GUEST_KERNEL_BUILD=release
suites="fxfs-guest-dax fxfs-guest-crash-dax fxfs-guest-checks-dax fxfs-guest-checks-dax-delay"

# Host A: the suites, rounds of each.
FXDAX_E2E_TEST=$suites FXDAX_E2E_ROUNDS=$rounds \
    scripts/test-gce.sh run "$host_a" "$results/suites" > "$results/suites.log" 2>&1 &
suites_pid=$!

# Host B: pgbench-dax, then every mutant, each of which a round of the suites must kill.
(
    FXDAX_E2E_TEST=fxfs-guest-pgbench-dax FXDAX_E2E_ROUNDS=1 \
        scripts/test-gce.sh run "$host_b" "$results/pgbench" > "$results/pgbench.log" 2>&1
    echo "pgbench-dax passed"
    FXDAX_E2E_TEST="fxfs-guest-dax fxfs-guest-checks-dax fxfs-guest-checks-dax-delay" \
        FXDAX_E2E_ROUNDS=$mutant_rounds \
        scripts/guest-mutants-gce.py "$host_b" "$zone" "$results/mutants" \
        | tee "$results/mutants.txt"
) > "$results/host-b.log" 2>&1 &
host_b_pid=$!

failed=()
wait "$suites_pid" || failed+=("the suites ($results/suites.log)")
wait "$host_b_pid" || failed+=("pgbench-dax ($results/pgbench.log)")
if [[ -f $results/mutants.txt ]] && grep -q '^MISSED' "$results/mutants.txt"; then
    failed+=("mutants missed: $(grep '^MISSED' "$results/mutants.txt" | cut -d: -f1 | tr '\n' ' ')")
fi
if (( ${#failed[@]} )); then
    printf 'dax-gate: FAILED: %s\n' "${failed[@]}" >&2
    exit 1
fi
git notes --ref=dax-gate add -f -m "passed $(date -u +%FT%TZ): $results" "$commit"
echo "dax-gate: passed, noted on $commit"
