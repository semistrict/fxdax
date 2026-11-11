#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on an x86-64 Linux host of one's own with KVM, over ssh (FXDAX_HOST, ubu1 by
# default; scripts/lib/host.sh): a worktree there of the commit, which origin has, runs the
# workspace's tests and fxdax's guest tests (guest/tests), in its Firecracker, whose DAX window
# takes KVM's faults through userfaultfd (scripts/lib/test-linux-host.sh), under fxjob; the results
# come back to RESULTS (default: a temporary directory), and the worktree goes.
#
# Usage: test-host.sh [RESULTS]
#        test-host.sh resume RUN RESULTS
#        test-host.sh cancel RUN
#
# resume waits for the run RUN, as the host names it (run.txt's in its results), whose driver
# stopped waiting for it, and fetches its results as a run would; cancel ends it (host_actions).
#
# FXDAX_E2E_TEST names the guest tests' cases to run, separated by spaces, by name or
# `--tag SUITE`, as scripts/test-e2e.sh's arguments, instead of its default suite, each
# FXDAX_E2E_ROUNDS times, under Firecracker's seccomp filter with FXDAX_FIRECRACKER_SECCOMP, in its
# jailer's jail with FXDAX_FIRECRACKER_JAIL (scripts/test-e2e.sh), fxfs-guest-checks' checks alone
# with FXDAX_CHECKS (such as `sparse,acls`), their budgets scaled by FXDAX_CHECKS_SCALE (such as 2,
# on a host others load). FXDAX_GUEST_KERNEL names fxdax's x86-64 guest kernel here
# (scripts/build-guest-kernel.sh), for the guest Fxfs tests, or FXDAX_GUEST_KERNEL_BUILD (release,
# debug, kasan, coverage or rust-checks) has the host build one from the source, or take the one
# built before from the same sources (host_kernel, scripts/lib/linux-host-setup.sh), a release one
# if neither is set; FXDAX_GUEST_CMDLINE adds to its arguments (such as
# fxfs.follow_apply_delay_us=200). FXDAX_HOST_LIMIT caps the run's time on the host (timeout's
# DURATION, default 6h). FXDAX_WORKSPACE_TESTS=0 runs the guest tests alone, FXDAX_GUEST_TESTS=0
# the workspace's alone, and FXDAX_TEST_ARGS are cargo nextest's arguments for the workspace's,
# such as a filter. FXDAX_EXTENTD_REMOTE, if set, names a bucket the guest tests' extent daemons
# keep their stores' branches in (an object store's URL the host reaches), under a prefix of the
# run's own. Every other FXDAX_* and PJDFSTEST_* variable set here reaches the host too
# (scripts/lib/host.sh), such as FXDAX_FOLLOW_SECONDS and FXDAX_PJDFSTEST_TESTS, and the run's
# FXDAX_HOST_* settings are scripts/lib/host.sh's.
set -euo pipefail
[[ ${FXDAX_E2E_TEST:-} =~ ^[a-z0-9\ -]*$ ]] || { echo "FXDAX_E2E_TEST selects test cases" >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
limit=6h

# finish STATUS says how the run went, its last lines, up to four, and returns STATUS.
finish() {
    local lines
    lines=$(wc -l < "$results/remote.log")
    sed -n "$((lines > 4 ? lines - 3 : 1)),\$p" "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$1"
}
host_after=finish
host_actions "$@"
(( $# <= 1 )) || { echo "Usage: $0 [RESULTS] | resume RUN RESULTS | cancel RUN" >&2; exit 2; }
results=${1:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-host.XXXXXX")}

status=0
host_start
echo "Results: $results" >&2
# The run's own prefix of the bucket, if one is named.
remote=
if [[ -n ${FXDAX_EXTENTD_REMOTE:-} ]]; then remote=${FXDAX_EXTENTD_REMOTE%/}/$host_run; fi
# shellcheck disable=SC2086 # The tests' selectors are words separated by spaces.
host_job "FXDAX_EXTENTD_REMOTE=$remote" test-linux-host.sh ${FXDAX_E2E_TEST:-} \
    > "$results/remote.log" 2>&1 || status=$?
if [[ $host_state == ended ]]; then
    host_fetch || status=$?
fi
finish "$status"
