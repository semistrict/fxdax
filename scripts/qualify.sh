#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax as LangSmith's sandboxes run it, before a pin is handed to them (TASK-330): the
# sandboxes' run (scripts/sandboxes.sh, TASK-330.1) in FXDAX_QUALIFY_SANDBOXES's mode (long, the
# default, a quarter hour on ubu1 under fxjob --bench; or short), then guests writing all the while
# against a bucket that slows, fails and answers again, for FXDAX_QUALIFY_SOAK_SECS seconds
# (fxfs-guest-bucket-soak, TASK-330.3, RustFS's bucket through a degrading proxy; default 1800,
# 1200 at least; 0 for none); every extent daemon of both held to its design's budgets
# (scripts/budgets.py, TASK-330.2). On Linux with KVM, as root, as
# scripts/test-e2e.sh runs guests; what that takes from the environment passes through, such as
# FXDAX_GUEST_KERNEL, and FXDAX_E2E_PREBUILT with FXDAX_E2E_HARNESS, this tree's harness driving
# another's build (a pin's from before the cases). FXDAX_SANDBOX_DISK is the sandboxes' disk,
# ext4 (default: RESULTS's).
#
#   qualify.sh RESULTS
#
# RESULTS gets each part's results (sandboxes/, soak/) and what was measured, as the fs bench's
# results are (bench-*.json): the cases' measurements and the checks they failed, and the
# daemons' budgets' (bench-budgets.json, scripts/qualify-budgets.py), which scripts/bench-ab.py
# compares, a pin's rounds against the next's (scripts/bench-ab-host.sh's FXDAX_AB_SUITE=qualify,
# TASK-330.4). Not the stores, images and caches a failed case leaves, gigabytes of them. Exits 0
# if every part passed, 1 if one failed, 2 if it couldn't start.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
results=${1:?usage: qualify.sh RESULTS}
mode=${FXDAX_QUALIFY_SANDBOXES:-long}
soak_secs=${FXDAX_QUALIFY_SOAK_SECS:-1800}
[[ $mode == short || $mode == long ]] ||
    { echo "FXDAX_QUALIFY_SANDBOXES is short or long" >&2; exit 2; }
[[ $soak_secs =~ ^[0-9]+$ ]] || { echo "FXDAX_QUALIFY_SOAK_SECS is a count of seconds" >&2; exit 2; }
(( soak_secs == 0 || soak_secs >= 1200 )) ||
    { echo "FXDAX_QUALIFY_SOAK_SECS is 0 or 1200 at least (fxfs-guest-bucket-soak)" >&2; exit 2; }
mkdir -p "$results"
results=$(cd "$results" && pwd)
# What it measured is every bench-*.json there: an earlier run's would be this one's.
if compgen -G "$results/bench-*.json" > /dev/null; then
    echo "$results holds an earlier run's results: name a new directory." >&2
    exit 2
fi
export FXDAX_BENCH_RESULTS=$results
export FXDAX_SANDBOX_DISK=${FXDAX_SANDBOX_DISK:-$results}

status=0 parts=(sandboxes)
echo "== sandboxes ($mode)" >&2
FXDAX_E2E_RESULTS="$results/sandboxes" "$root/scripts/sandboxes.sh" "$mode" || status=1
if (( soak_secs > 0 )); then
    parts+=(soak)
    echo "== bucket soak (${soak_secs} s)" >&2
    # Its bucket RustFS's, which the case degrades through a proxy of its own, its objects on the
    # results' disk.
    soak_bucket=$(mktemp -d "$results/soak-bucket.XXXXXX")
    FXDAX_BUCKET_SOAK_SECS=$soak_secs FXDAX_E2E_RESULTS="$results/soak" \
        FXDAX_RUSTFS_DATA="$soak_bucket" FXDAX_RUSTFS_LOG="$results/soak-rustfs.log" \
        "$root/scripts/test-e2e-rustfs.sh" fxfs-guest-bucket-soak || status=1
    rm -r "$soak_bucket"
fi
"$root/scripts/qualify-budgets.py" "$results" "${parts[@]}" || status=1
# What a failed case leaves of its stores, buckets, caches, images and roots: not results.
find "$results" -mindepth 2 -type d \( -name '*.root' -o -name '*.data' -o -name '*.base' \
    -o -name '*-cache' -o -name '.extentd-cache.*' -o -name '*.bucket' -o -name 'store-*' \
    -o -name sandbox-image -o -name rootfs -o -name extents \) -prune -exec rm -rf {} +
find "$results" -type f \( -name '*.ext4' -o -name '*.img' \) -delete
echo "qualify: exit $status, results in $results" >&2
exit "$status"
