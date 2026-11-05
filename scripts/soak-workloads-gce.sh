#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Soaks the guest suites on disposable GCE hosts, a host for each group of workloads at once, each
# case many rounds, stopping at its first failing round, whose logs its results keep
# (scripts/test-e2e.sh); a group's cases are those of its suite, `runner list --tag soak-GROUP`:
#
#   crash     the VM stopped mid-workload, its process killed
#   checks    the guest checks, with delays and on a small window
#   sanitized the DAX checks and allocations failing, on the debug kernel (lockdep, KCSAN), too
#             slow for the delays' and the small window's within their cases' timeouts
#   replicas  followers under torture and remounting, snapshots, suspending and migrating
#   shapes    DAX pgbench over guest memory, vCPUs and windows (scripts/soak-gce.sh)
#
#   scripts/soak-workloads-gce.sh RESULTS [GROUP...]
#
# runs every group, or those named, each in RESULTS/GROUP, and prints how each went. A project's
# CPU quota bounds the hosts it can have at once. FXDAX_SOAK_ROUNDS scales each group's rounds by
# a factor (1 by default), and every FXDAX_* setting scripts/test-gce.sh takes reaches each host.
# A host pushes the working tree as its run begins: run from a tree that doesn't change meanwhile.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
results=${1:?usage: soak-workloads-gce.sh RESULTS [GROUP...]}
shift
groups=("$@")
if (( ${#groups[@]} == 0 )); then
    groups=(crash checks sanitized replicas shapes)
fi
for group in "${groups[@]}"; do
    [[ $group =~ ^(crash|checks|sanitized|replicas|shapes)$ ]] || { echo "no such group: $group" >&2; exit 2; }
done
factor=${FXDAX_SOAK_ROUNDS:-1}
[[ $factor =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_SOAK_ROUNDS is a factor" >&2; exit 2; }

# Runs `group`'s suite on a host of its own: its rounds, its kernel, its seconds of following.
soak() {
    local group=$1 rounds=$2 kernel=$3 seconds=$4
    FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST="--tag soak-$group" \
        FXDAX_E2E_ROUNDS=$((rounds * factor)) FXDAX_GUEST_KERNEL_BUILD=$kernel \
        FXDAX_FOLLOW_SECONDS=$seconds \
        "$repo/scripts/test-gce.sh" all "fxdax-test-soak-$group-$(date -u +%m%d%H%M%S)" \
        "$results/$group" > "$results/$group.log" 2>&1
}

mkdir -p "$results"
results=$(cd "$results" && pwd)
pids=()
for group in "${groups[@]}"; do
    case $group in
        crash) soak crash 50 release 20 & ;;
        checks) soak checks 10 release 20 & ;;
        sanitized) soak sanitized 10 debug 20 & ;;
        replicas) soak replicas 5 release 600 & ;;
        shapes)
            FXDAX_SOAK_ROUNDS=$((3 * factor)) "$repo/scripts/soak-gce.sh" "$results/shapes" \
                > "$results/shapes.log" 2>&1 &
            ;;
    esac
    pids+=("$!")
done
status=0
for i in "${!groups[@]}"; do
    if wait "${pids[$i]}"; then
        echo "${groups[$i]}: passed"
    else
        status=1
        echo "${groups[$i]}: failed, see $results/${groups[$i]}.log"
    fi
done
exit "$status"
