#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The sandbox team's performance gate, and copy-on-write DAX's, on a GCE host
# (TASK-131.5, TASK-164.5). One VM a case (RESULTS/single): pgbench, five runs
# of a VM, on the guest's Fxfs without a DAX window, and with one, whose stores
# copy on write, each VM making its database; the same two on a database a
# base holds, which the VM forks; on ext4 with DAX on virtio-pmem, a static DAX
# device; and bigmap, a file four times the guest's memory, in a base, mapped
# shared, read all over and 5% of it written, the same two ways. Then the
# density run (RESULTS/density): FXDAX_PERF_GATE_VMS VMs (default 3) on forks
# of a base holding the database, without DAX and with it, all served by one
# daemon, each making FXDAX_PERF_GATE_DENSITY_RUNS runs (default 2) and kept
# running until the last's are done, the host's memory measured as each is
# added. Then scripts/pgbench-gate.py over both, which writes
# RESULTS/gate.json. It fails if a run or a gate does.
# docs/measurements/dax-cow-gate.md says what each case measures.
#
# Usage: perf-gate-gce.sh [--bare-metal] INSTANCE RESULTS
#
# INSTANCE is a host scripts/test-gce.sh made (`test-gce.sh create`), of the
# kind the numbers are reported as: GCE's VMs, nested, by default; with
# --bare-metal, a metal machine type (FXDAX_GCE_MACHINE). Numbers of the two
# kinds are not compared. A release guest kernel unless FXDAX_GUEST_KERNEL_BUILD
# says otherwise; FXDAX_GATE_* are the gates' thresholds (pgbench-gate.py), and
# every other FXDAX_* reaches the host (test-gce.sh), FXDAX_GUEST_CMDLINE among
# them.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
host=nested
if [[ ${1:-} == --bare-metal ]]; then
    host=bare-metal
    shift
fi
[[ $# == 2 ]] || { echo "Usage: $0 [--bare-metal] INSTANCE RESULTS" >&2; exit 2; }
instance=$1 results=$2
single="fxfs-guest-pgbench fxfs-guest-pgbench-dax pgbench-ext4-dax-pmem"
single+=" fxfs-guest-pgbench-forked fxfs-guest-pgbench-dax-forked"
single+=" fxfs-guest-bigmap fxfs-guest-bigmap-dax"
density="fxfs-guest-pgbench-density fxfs-guest-pgbench-dax-density"
export FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_ROUNDS=1
export FXDAX_GUEST_KERNEL_BUILD=${FXDAX_GUEST_KERNEL_BUILD:-release}
status=0
FXDAX_E2E_TEST=$single "$repo/scripts/test-gce.sh" run "$instance" "$results/single" || status=$?
FXDAX_PGBENCH_VMS=${FXDAX_PERF_GATE_VMS:-3} \
    FXDAX_PGBENCH_RUNS=${FXDAX_PERF_GATE_DENSITY_RUNS:-2} FXDAX_E2E_TEST=$density \
    "$repo/scripts/test-gce.sh" run "$instance" "$results/density" || status=$?
"$repo/scripts/pgbench-gate.py" --host "$host" --json "$results/gate.json" \
    "$results/single" "$results/density" || status=$?
exit "$status"
