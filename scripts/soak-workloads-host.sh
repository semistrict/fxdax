#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Soaks the guest suites on a Linux host of one's own, a run of scripts/test-host.sh for each group
# of workloads, one after another, as others' runs share the host; each case many rounds, stopping
# at its first failing round, whose logs its results keep (scripts/test-e2e.sh); a group's cases are
# those of its suite, `runner list --tag soak-GROUP`:
#
#   crash     the VM stopped mid-workload, its process killed
#   checks    the guest checks, with delays and on a small window
#   sanitized the DAX checks and allocations failing, on the debug kernel (lockdep, KCSAN), too
#             slow for the delays' and the small window's within their cases' timeouts
#   replicas  followers under torture and remounting, snapshots, suspending and migrating
#   shapes    DAX pgbench over guest memory, vCPUs and windows (scripts/soak-host.sh)
#   mixed     VMs on forks of a base under writers, fsx-style scripts and pgbench for hours, each
#             round through VMs killed, the daemon killed, uploads failing and late, the host or
#             its cache lost, and checked against doc-12 (fxfs-guest-soak, TASK-67)
#
#   scripts/soak-workloads-host.sh RESULTS [GROUP...]
#
# runs every group, or those named, each in RESULTS/GROUP, and prints how each went, and each case
# of its suite, from its summary (scripts/guest_results.py): its rounds, or the round it failed in,
# its checks failed, and the command that replays it. FXDAX_SOAK_ROUNDS scales each group's rounds
# by a factor (1 by default), and every FXDAX_* setting scripts/test-host.sh takes reaches the
# host. Each run is of the commit scripts/test-host.sh runs (scripts/lib/host.sh's
# FXDAX_HOST_COMMIT, HEAD by default).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
results=${1:?usage: soak-workloads-host.sh RESULTS [GROUP...]}
shift
groups=("$@")
if (( ${#groups[@]} == 0 )); then
    groups=(crash checks sanitized replicas shapes mixed)
fi
for group in "${groups[@]}"; do
    [[ $group =~ ^(crash|checks|sanitized|replicas|shapes|mixed)$ ]] || { echo "no such group: $group" >&2; exit 2; }
done
factor=${FXDAX_SOAK_ROUNDS:-1}
[[ $factor =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_SOAK_ROUNDS is a factor" >&2; exit 2; }

# Runs `group`'s suite: its rounds, its kernel, its seconds of following. The mixed group's, some
# 5 minutes a round, its daemons built to fail uploads.
soak() {
    local group=$1 rounds=$2 kernel=$3 seconds=$4 own=()
    if [[ $group == mixed ]]; then
        own=(FXDAX_EXTENTD_FAILPOINTS=1)
    fi
    env ${own[@]+"${own[@]}"} FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST="--tag soak-$group" \
        FXDAX_E2E_ROUNDS=$((rounds * factor)) FXDAX_GUEST_KERNEL_BUILD=$kernel \
        FXDAX_FOLLOW_SECONDS=$seconds \
        "$repo/scripts/test-host.sh" "$results/$group" > "$results/$group.log" 2>&1
}

mkdir -p "$results"
results=$(cd "$results" && pwd)
status=0
for group in "${groups[@]}"; do
    soaked=0
    case $group in
        crash) soak crash 50 release 20 || soaked=$? ;;
        checks) soak checks 10 release 20 || soaked=$? ;;
        sanitized) soak sanitized 10 debug 20 || soaked=$? ;;
        replicas) soak replicas 5 release 600 || soaked=$? ;;
        mixed) soak mixed 40 release 0 || soaked=$? ;;
        shapes)
            FXDAX_SOAK_ROUNDS=$((3 * factor)) "$repo/scripts/soak-host.sh" "$results/shapes" \
                > "$results/shapes.log" 2>&1 || soaked=$?
            ;;
    esac
    if (( soaked == 0 )); then
        echo "$group: passed"
    else
        status=1
        echo "$group: failed, see $results/$group.log"
    fi
    # shapes' runs are soak-host.sh's, each its own.
    if [[ $group != shapes ]]; then
        "$repo/scripts/guest_results.py" report "$results/$group/e2e" | sed 's/^/  /' || true
    fi
done
exit "$status"
