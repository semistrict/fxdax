#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run fxfs-guest-sandboxes (TASK-330.1): sandboxes as LangSmith's CI runs them, two extent daemons
# and their S3 bucket on one ext4 disk, many VMs at once on too few CPUs, building and exporting
# Docker images, running pgbench and working git repositories (guest/tests' case says what it
# checks). On Linux with KVM, as scripts/test-e2e.sh runs guests.
#
#   sandboxes.sh [short|long]
#
# short (the default) is for every push, minutes; long, LangSmith's 16 VMs, eight at a time, a
# quarter hour, is the qualification's before each pin, on ubu1 (scripts/qualify.sh). The disk is
# FXDAX_SANDBOX_DISK's (default: TMPDIR's), which must be ext4: the run's directory there holds the
# bucket's data, which RustFS serves (scripts/test-e2e-rustfs.sh, which runs the case), and
# scripts/test-e2e.sh's results (FXDAX_E2E_RESULTS, by default in the run's directory), the case's
# stores and the daemons' part caches among them; and RustFS's log (rustfs.log) and what the host
# and the run spend (resources.log, scripts/lib/resources.sh). What test-e2e.sh takes from the
# environment passes through (FXDAX_GUEST_KERNEL, FXDAX_E2E_PREBUILT). The bucket's data goes at
# the end, and the run's directory with it unless it holds the results.
#
# Exits 0 if the run passed, 1 if it failed (test-e2e.sh's status), 2 if it couldn't be set up.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/lib/resources.sh"
mode=${1:-short}
case $mode in
    short | long) ;;
    *) echo "usage: sandboxes.sh [short|long]" >&2; exit 2 ;;
esac
disk=${FXDAX_SANDBOX_DISK:-${TMPDIR:-/tmp}}
fstype=$(findmnt -no FSTYPE -T "$disk")
if [[ $fstype != ext4 ]]; then
    echo "$disk is on $fstype: the sandboxes' disk is ext4, as LangSmith's (FXDAX_SANDBOX_DISK)" >&2
    exit 2
fi
work=$(mktemp -d "$disk/fxdax-sandboxes.XXXXXX")
bucket_data="$work/bucket"
mkdir "$bucket_data"
results=${FXDAX_E2E_RESULTS:-$work/results}
mkdir -p "$results"
# What the host and the run spend, every 30 s, in the results: the long mode ran a CI runner out of
# memory and down, and ran on ubu1 past 2 hours with nothing kept to say why (TASK-330.5).
resources_watch "$results/resources.log"

cleanup() {
    kill "$resources_watcher"
    rm -r "$bucket_data"
    # The run's directory, and what a case that failed left in it, its daemons' sockets among
    # them, unless it holds the results.
    [[ $results == "$work"/* ]] || rm -r "$work"
}
trap cleanup EXIT

# Its bucket's objects on the disk, as LangSmith's RustFS kept them beside the hosts' stores
# (scripts/test-e2e-rustfs.sh).
status=0
# TMPDIR left as it is, short: the cases' sockets are there (the runner refuses a long one).
FXDAX_E2E_RESULTS="$results" FXDAX_RUSTFS_DATA="$bucket_data" \
    FXDAX_RUSTFS_LOG="$results/rustfs.log" \
    FXDAX_SANDBOX_BUCKET_DIR="$bucket_data" FXDAX_SANDBOX_MODE="$mode" \
    "$root/scripts/test-e2e-rustfs.sh" fxfs-guest-sandboxes || status=$?
# What the case said of its run, and each check it failed, from its log: the runner says only
# that it failed.
find "$results" -name log.txt -path '*fxfs-guest-sandboxes*' -exec grep -h '^sandboxes: ' {} + >&2 ||
    true
echo "sandboxes ($mode): exit $status, results in $results" >&2
exit "$status"
