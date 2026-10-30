#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The sandbox team's performance gate, on a GCE host (TASK-131.5): pgbench, five
# runs of a VM, on the guest's Fxfs without a DAX window and with one, and on
# ext4 with DAX on virtio-pmem, a static DAX device (RESULTS/single); then
# FXDAX_PERF_GATE_VMS VMs (default 3) one after another, all served by one
# daemon, with a DAX window (RESULTS/density); then scripts/pgbench-gate.py over
# both, which writes RESULTS/gate.json. It fails if a run or a gate does.
#
# Usage: perf-gate-gce.sh [--bare-metal] INSTANCE RESULTS
#
# INSTANCE is a host scripts/test-gce.sh made (`test-gce.sh create`), of the
# kind the numbers are reported as: GCE's VMs, nested, by default; with
# --bare-metal, a metal machine type (FXDAX_GCE_MACHINE). Numbers of the two
# kinds are not compared. A release guest kernel unless FXDAX_GUEST_KERNEL_BUILD
# says otherwise; FXDAX_GATE_* are the gates' thresholds (pgbench-gate.py), and
# every other FXDAX_* reaches the host (test-gce.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
host=nested
if [[ ${1:-} == --bare-metal ]]; then
    host=bare-metal
    shift
fi
[[ $# == 2 ]] || { echo "Usage: $0 [--bare-metal] INSTANCE RESULTS" >&2; exit 2; }
instance=$1 results=$2
export FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_ROUNDS=1
export FXDAX_GUEST_KERNEL_BUILD=${FXDAX_GUEST_KERNEL_BUILD:-release}
status=0
FXDAX_E2E_TEST="fxfs-guest-pgbench fxfs-guest-pgbench-dax pgbench-ext4-dax-pmem" \
    "$repo/scripts/test-gce.sh" run "$instance" "$results/single" || status=$?
FXDAX_PGBENCH_VMS=${FXDAX_PERF_GATE_VMS:-3} FXDAX_E2E_TEST=fxfs-guest-pgbench-dax \
    "$repo/scripts/test-gce.sh" run "$instance" "$results/density" || status=$?
"$repo/scripts/pgbench-gate.py" --host "$host" --json "$results/gate.json" \
    "$results/single" "$results/density" || status=$?
exit "$status"
