#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on a disposable x86-64 GCE host with nested KVM
# (scripts/lib/gce.sh). It runs the workspace's tests and fxdax's guest tests
# (guest/tests), in its Firecracker, whose DAX window takes KVM's faults through
# userfaultfd. `all`
# always deletes its host; `create`, `run` and `delete` expose the same steps
# for interrupted runs.
#
# Usage: test-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), and FXDAX_E2E_TEST the guest tests' cases to
# run, separated by spaces, instead of scripts/test-e2e.sh's (`all` runs every one),
# each FXDAX_E2E_ROUNDS times, under Firecracker's seccomp filter with
# FXDAX_FIRECRACKER_SECCOMP, in its jailer's jail with FXDAX_FIRECRACKER_JAIL
# (scripts/test-e2e.sh), fxfs-guest-checks' checks alone with FXDAX_CHECKS
# (such as `sparse,acls`). FXDAX_GUEST_KERNEL
# names fxdax's x86-64 guest kernel (scripts/build-guest-kernel.sh), for the
# guest Fxfs tests, or FXDAX_GUEST_KERNEL_BUILD (release or debug) has the host
# build one from the source (scripts/lib/test-linux-host.sh); FXDAX_GUEST_CMDLINE
# adds to its arguments (such as fxfs.follow_apply_delay_us=200). FXDAX_GCE_LIMIT
# caps a run's time on the host (timeout's DURATION, default 6h).
# FXDAX_WORKSPACE_TESTS=0 runs the guest tests alone, FXDAX_GUEST_TESTS=0 the
# workspace's alone, and FXDAX_TEST_ARGS are cargo
# nextest's arguments for the workspace's, such as a filter. FXDAX_EXTENTD_REMOTE,
# if set, names a bucket (gs://bucket/prefix) the guest tests' extent daemons
# keep their stores' branches in, under a prefix of the run's own: the host, made
# with FXDAX_GCE_STORAGE set, reaches it (scripts/lib/gce.sh). Every other
# FXDAX_* and PJDFSTEST_* variable set here reaches the host too
# (scripts/lib/gce.sh), such as FXDAX_FOLLOW_SECONDS and FXDAX_PJDFSTEST_TESTS.
set -euo pipefail
[[ ${FXDAX_E2E_TEST:-} =~ ^[a-z0-9\ -]*$ ]] || { echo "FXDAX_E2E_TEST names test cases" >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
action=${1:-all}
instance=${2:-fxdax-test-$(date -u +%Y%m%d-%H%M%S)-$gce_maker}
results=${3:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce.XXXXXX")}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-test-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-test- instance name.' >&2; exit 2; }
machine=n2-standard-8
purpose=fxdax-test
# The host deletes itself after this long; the run's own limit leaves time to
# copy the results back.
lifetime=8h limit=${FXDAX_GCE_LIMIT:-6h}
create_flags=(--min-cpu-platform='Intel Cascade Lake')

run() {
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    gce_wait_ready
    # Each run unpacks into a directory of its own, so that no earlier run's
    # files or recorded times carry over.
    local run status=0
    run="run-$(date -u +%Y%m%dT%H%M%SZ)"
    gce_push_source "$run"
    # The run's own prefix of the bucket, if one is named.
    local remote=
    if [[ -n ${FXDAX_EXTENTD_REMOTE:-} ]]; then remote=${FXDAX_EXTENTD_REMOTE%/}/$run-$gce_maker; fi
    # shellcheck disable=SC2086 # The tests are names separated by spaces.
    gce_remote_detached "$run" "$(gce_host_command "$run" "FXDAX_EXTENTD_REMOTE=$remote" \
        test-linux-host.sh ${FXDAX_E2E_TEST:-})" > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    # The last lines, which say how the tests went: up to four.
    local lines
    lines=$(wc -l < "$results/remote.log")
    sed -n "$((lines > 4 ? lines - 3 : 1)),\$p" "$results/remote.log" >&2
    echo "Results: $results" >&2
    return "$status"
}

case "$action" in
    create) mkdir -p "$results"; gce_create ;;
    run) gce_locate; run ;;
    delete) gce_locate; gce_delete ;;
    all) mkdir -p "$results"; gce_all run ;;
    *) echo "Usage: $0 [all|create|run|delete] [instance] [results-directory]" >&2; exit 2 ;;
esac
