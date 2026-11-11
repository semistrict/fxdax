#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Production-shaped soaks of the guest Fxfs on a Linux host of one's own (TASK-44.18): DAX pgbench
# (fxfs-guest-pgbench-dax) over a matrix of guest memory, vCPUs and DAX window sizes, its bucket
# FXDAX_REMOTE_LATENCY_MS away (25 by default), FXDAX_SOAK_ROUNDS rounds a cell (3), as the
# sandbox team's small VMs meet what the suites' 4 GiB, 2-vCPU, 2 GiB-window guest doesn't
# (TASK-99 was a 2 GiB VM's). Each cell a run of scripts/test-host.sh, one after another; each
# cell's results, each round's guest log among them (round-K.log), go to
# RESULTS/<memory>m-<vcpus>c-<window>w, and a table of every round's tps, or of where it failed, to
# standard output.
#
#   scripts/soak-host.sh RESULTS [MEMORY_MIB:VCPUS:WINDOW_MIB...]
#
# The default matrix: 1024:2:2048 1024:4:2048 2048:4:512 2048:4:8192 4096:2:2048. The host, the
# guest kernel and every other setting are scripts/test-host.sh's (FXDAX_HOST, FXDAX_GUEST_KERNEL
# or FXDAX_GUEST_KERNEL_BUILD, and every other FXDAX_* reaching the host).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
results=${1:?usage: soak-host.sh RESULTS [MEMORY_MIB:VCPUS:WINDOW_MIB...]}
shift
cells=("$@")
if (( ${#cells[@]} == 0 )); then
    cells=(1024:2:2048 1024:4:2048 2048:4:512 2048:4:8192 4096:2:2048)
fi
for cell in "${cells[@]}"; do
    [[ $cell =~ ^[0-9]+:[0-9]+:[0-9]+$ ]] || { echo "a cell is MEMORY_MIB:VCPUS:WINDOW_MIB: $cell" >&2; exit 2; }
done
mkdir -p "$results"
results=$(cd "$results" && pwd)
export FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-pgbench-dax
export FXDAX_E2E_ROUNDS=${FXDAX_SOAK_ROUNDS:-3} FXDAX_REMOTE_LATENCY_MS=${FXDAX_REMOTE_LATENCY_MS:-25}
status=0
for cell in "${cells[@]}"; do
    IFS=: read -r memory vcpus window <<< "$cell"
    out="$results/${memory}m-${vcpus}c-${window}w"
    FXDAX_PGBENCH_MEMORY_MIB=$memory FXDAX_PGBENCH_VCPUS=$vcpus FXDAX_PGBENCH_WINDOW_MIB=$window \
        "$repo/scripts/test-host.sh" "$out" > "$out.log" 2>&1 || status=1
    # Each round's tps, its five runs', or how the cell failed.
    for log in "$out"/e2e/fxfs-guest-pgbench-dax/round-*.log; do
        [[ -f $log ]] || continue
        tps=$(sed -n 's/^tps = \([0-9]*\)\..*/\1/p' "$log" | tr '\n' ' ')
        echo "$cell $(basename "$log" .log): $tps"
    done
    if grep -q "failed in round" "$out/e2e.log" 2>/dev/null; then
        echo "$cell: $(grep -m1 "failed in round" "$out/e2e.log")"
    fi
done
exit "$status"
