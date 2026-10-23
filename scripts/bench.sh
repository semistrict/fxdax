#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Benchmark the guest kernel's Fxfs against ext4 on this host: guests in fxdax's
# Firecracker boot fxdax's guest kernel (FXDAX_GUEST_KERNEL) and run the same
# workloads on its Fxfs, on an extent-store device that fxdax's extent daemon
# serves, with a DAX window and without, and on ext4 on the same kernel, on
# virtio-blk and with DAX on writable virtio-pmem
# (guest/tests/test_cases/src/test_fs_bench.rs), then scripts/bench-report.py
# compares what they measured: what the cases that passed did, if others failed.
# It fails if a case or the report did, with the cases' status if they did.
#
# Usage: scripts/bench.sh [ITERATIONS]   (default 3)
#
# FXDAX_BENCH_CASES names the cases to run, separated by spaces (default: all),
# and FXDAX_BENCH_GROUPS the groups of workloads (default: all; see
# test_fs_bench.rs).
#
# Needs what scripts/test-e2e.sh needs, and podman to build the guests' Alpine
# root filesystem. The measurements and the guests' logs are kept under
# $FXDAX_BENCH_RESULTS (default: a new temporary directory), which holds no
# earlier run's.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
iterations=${1:-3}
cases=${FXDAX_BENCH_CASES:-bench-fs-ext4-guest-kernel bench-fs-ext4-dax-pmem
    bench-fs-fxfs-guest bench-fs-fxfs-guest-dax}
results=${FXDAX_BENCH_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-bench.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)
# The report is of every measurement and log there: an earlier run's would be
# this one's, those of a case that failed now, which writes none, among them.
if compgen -G "$results/bench-*.json" > /dev/null || [[ -e $results/logs ]]; then
    echo "$results holds an earlier run's results: name a new directory in FXDAX_BENCH_RESULTS." >&2
    exit 2
fi

status=0 report=0
# shellcheck disable=SC2086 # The cases are names separated by spaces.
FXDAX_BENCH_ITERATIONS=$iterations FXDAX_BENCH_RESULTS=$results \
    FXDAX_E2E_RESULTS=$results/logs "$root/scripts/test-e2e.sh" $cases >&2 || status=$?
echo "Results: $results" >&2
"$root/scripts/bench-report.py" "$results" || report=$?
exit $(( status ? status : report ))
