#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The gate a change to the guest's DAX code passes before it is pushed (TASK-119.8): two runs of
# HEAD, a commit origin has, at once on a Linux host of one's own (scripts/test-host.sh;
# FXDAX_HOST, ubu1 by default). The first runs HEAD's DAX suites, rounds of each: the checks with
# delays injected (fxfs-guest-checks-dax-delay) and on a small window (-small), and the crash test
# and its variant killing its process (-killed). The second runs pgbench-dax-forked, on a base's
# database, whose files are DAX and whose stores copy on write, whose data is checked and whose
# write path's latencies stay under their ceilings (scripts/pgbench-probes.py): a database made in
# the run is files created in the mount, which the page cache writes (doc-13); then every hand
# mutant (guest/mutants) that needs a real kernel (scripts/guest-mutants-host.py), each killed by
# the killer its header names within FXDAX_GATE_MUTANT_ROUNDS rounds (default 3). Before either run
# starts, the simulator's mutants, in minutes (guest/sim/mutants.sh, through guest/sim/linux.sh, so
# on Linux with KERNEL and BUILD set, or in a container elsewhere). A pass is recorded as a note on
# HEAD (`git notes --ref=dax-gate`), which scripts/hooks/pre-push asks for of a push changing the
# guest's DAX code: `git config core.hooksPath scripts/hooks` installs it.
#
# Usage: dax-gate.sh
# FXDAX_GATE_ROUNDS is the suites' rounds (default 5). Results go to /tmp/fxdax-gate-<commit>.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
(( $# == 0 )) || { sed -n '6,22p' "$0" >&2; exit 2; }
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

# The simulator's mutants, each killed in seconds or not at all: what it misses fails the gate
# before the host spends hours.
if ! guest/sim/linux.sh ./mutants.sh > "$results/sim-mutants.txt" 2>&1; then
    echo "dax-gate: FAILED: the simulator's mutants ($results/sim-mutants.txt):" >&2
    grep -vE '^(fate|killed|none) ' "$results/sim-mutants.txt" >&2 || true
    exit 1
fi
echo "dax-gate: the simulator's mutants killed"

export FXDAX_HOST_COMMIT=$commit FXDAX_WORKSPACE_TESTS=0 FXDAX_GUEST_KERNEL_BUILD=release

# The first run: the suites, rounds of each (`runner list --tag dax-gate`).
FXDAX_E2E_TEST="--tag dax-gate" FXDAX_E2E_ROUNDS=$rounds \
    scripts/test-host.sh "$results/suites" > "$results/suites.log" 2>&1 &
suites_pid=$!

# The second: pgbench-dax-forked, then the mutants that need a real kernel, each its killer's case.
(
    FXDAX_E2E_TEST=fxfs-guest-pgbench-dax-forked FXDAX_E2E_ROUNDS=1 \
        scripts/test-host.sh "$results/pgbench" > "$results/pgbench.log" 2>&1
    echo "pgbench-dax-forked passed"
    pgbench=$results/pgbench/e2e/fxfs-guest-pgbench-dax-forked/fxfs-guest-pgbench-dax-forked
    scripts/pgbench-probes.py "$pgbench/run.console.txt"
    FXDAX_E2E_ROUNDS=$mutant_rounds \
        scripts/guest-mutants-host.py "$results/mutants" > "$results/mutants.txt"
) > "$results/second.log" 2>&1 &
second_pid=$!

# What failed: each run's cases, as its summary has them (scripts/guest_results.py).
failed=0
if ! wait "$suites_pid"; then
    echo "dax-gate: FAILED: the suites ($results/suites.log):" >&2
    scripts/guest_results.py report "$results/suites/e2e" >&2 || true
    failed=1
fi
if ! wait "$second_pid"; then
    echo "dax-gate: FAILED: the second run ($results/second.log); pgbench-dax-forked:" >&2
    scripts/guest_results.py report "$results/pgbench/e2e" >&2 || true
    failed=1
fi
if [[ -f $results/mutants.txt ]] && grep -vqE '^(killed|none) ' "$results/mutants.txt"; then
    echo "dax-gate: FAILED: mutants not killed ($results/mutants.txt):" >&2
    grep -vE '^(killed|none) ' "$results/mutants.txt" >&2
    failed=1
fi
if (( failed )); then
    exit 1
fi
git notes --ref=dax-gate add -f -m "passed $(date -u +%FT%TZ): $results" "$commit"
echo "dax-gate: passed, noted on $commit"
