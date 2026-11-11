#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The sandbox team's performance gate, and copy-on-write DAX's, on a Linux host of one's own
# (TASK-131.5, TASK-164.5), each run of scripts/test-host.sh a measurement there, alone and on CPUs
# of its own (FXDAX_HOST_BENCH=1). One VM a case (RESULTS/single): pgbench, five runs of a VM, on
# the guest's Fxfs without a DAX window, and with one, whose stores copy on write, each VM making
# its database; the same two on a database a base holds, which the VM forks; on ext4 with DAX on
# virtio-pmem, a static DAX device; on ext4 on virtio-blk, its flushes dropped, the sandbox team's
# runtime, and each a host fsync (TASK-91.8); and bigmap, a file four times the guest's memory, in
# a base, mapped shared, read all over and 5% of it written, the same two ways. Then the density
# run (RESULTS/density): FXDAX_PERF_GATE_VMS VMs (default 3) on forks of a base holding the
# database, without DAX and with it, all served by one daemon, each making
# FXDAX_PERF_GATE_DENSITY_RUNS runs (default 2) and kept running until the last's are done, the
# host's memory measured as each is added. Then scripts/pgbench-gate.py over both, which writes
# RESULTS/gate.json, and scripts/pgbench-steps.py, each case's runs step by step in RESULTS/steps.md
# (and steps.json). It fails if a run or a gate does. docs/measurements/dax-cow-gate.md says what
# each case measures.
#
# Usage: perf-gate-host.sh [--nested] RESULTS [PHASE...]
#
# The host (FXDAX_HOST, scripts/lib/host.sh) is of the kind the numbers are reported as: bare
# metal, as ubu1 is, by default; with --nested, a VM whose guests are nested. Numbers of the two
# kinds are not compared. The PHASEs, in order, of `single`, `density` and `gate`, all three by
# default: each a run of its own, `gate` reading both runs' results in RESULTS. A release guest
# kernel unless FXDAX_GUEST_KERNEL_BUILD says otherwise; each Firecracker in its jailer's jail, as
# production runs it, unless FXDAX_FIRECRACKER_JAIL is set empty (TASK-172: a jailed VM process's
# DAX window budgets its mappings as the host allows, as an unjailed one's does); FXDAX_GATE_* are
# the gates' thresholds (pgbench-gate.py), and every other FXDAX_* reaches the host
# (test-host.sh), FXDAX_GUEST_CMDLINE among them.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
usage() { echo "Usage: $0 [--nested] RESULTS [single|density|gate]..." >&2; exit 2; }
kind=bare-metal
if [[ ${1:-} == --nested ]]; then
    kind=nested
    shift
fi
(( $# >= 1 )) || usage
results=$1
shift
phases=("$@")
(( ${#phases[@]} > 0 )) || phases=(single density gate)
for phase in "${phases[@]}"; do
    [[ $phase =~ ^(single|density|gate)$ ]] || usage
done
# The suites (`runner list --tag perf-single`, `--tag perf-density`).
single="--tag perf-single"
density="--tag perf-density"
export FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_ROUNDS=1 FXDAX_HOST_BENCH=1
export FXDAX_GUEST_KERNEL_BUILD=${FXDAX_GUEST_KERNEL_BUILD:-release}
export FXDAX_FIRECRACKER_JAIL=${FXDAX_FIRECRACKER_JAIL-1}
status=0
for phase in "${phases[@]}"; do
    case $phase in
        single)
            FXDAX_E2E_TEST=$single "$repo/scripts/test-host.sh" "$results/single" || status=$?
            ;;
        density)
            FXDAX_PGBENCH_VMS=${FXDAX_PERF_GATE_VMS:-3} \
                FXDAX_PGBENCH_RUNS=${FXDAX_PERF_GATE_DENSITY_RUNS:-2} FXDAX_E2E_TEST=$density \
                "$repo/scripts/test-host.sh" "$results/density" || status=$?
            ;;
        gate)
            "$repo/scripts/pgbench-gate.py" --host "$kind" --json "$results/gate.json" \
                "$results/single" "$results/density" || status=$?
            # Each case's runs step by step, as the sandbox team's harness reports them (TASK-91.8).
            "$repo/scripts/pgbench-steps.py" --json "$results/steps.json" "$results/single" \
                "$results/density" > "$results/steps.md" || status=$?
            ;;
    esac
done
exit "$status"
