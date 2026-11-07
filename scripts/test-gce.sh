#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on a disposable x86-64 GCE host with nested KVM
# (scripts/lib/gce.sh). It runs the workspace's tests and fxdax's guest tests
# (guest/tests), in its Firecracker, whose DAX window takes KVM's faults through
# userfaultfd. `all`
# always deletes its host; `create`, `run` and `delete` expose the same steps
# for interrupted runs, and `resume` waits for a run whose waiter here was
# stopped, named as the host names it (RUN, of its ~/RUN.job), and fetches its
# results as `run` would.
#
# Usage: test-gce.sh [all|create|run|delete] [instance] [results-directory]
#        test-gce.sh resume instance results-directory run
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), FXDAX_GCE_MACHINE the machine type
# (default: n2-standard-4), and FXDAX_E2E_TEST the guest tests' cases to
# run, separated by spaces, by name or `--tag SUITE`, as scripts/test-e2e.sh's arguments,
# instead of its default suite, each FXDAX_E2E_ROUNDS times, under Firecracker's seccomp filter with
# FXDAX_FIRECRACKER_SECCOMP, in its jailer's jail with FXDAX_FIRECRACKER_JAIL
# (scripts/test-e2e.sh), fxfs-guest-checks' checks alone with FXDAX_CHECKS
# (such as `sparse,acls`), their budgets scaled by FXDAX_CHECKS_SCALE (such as 2, on a slower
# FXDAX_GCE_MACHINE). FXDAX_GUEST_KERNEL
# names fxdax's x86-64 guest kernel (scripts/build-guest-kernel.sh), for the
# guest Fxfs tests, or FXDAX_GUEST_KERNEL_BUILD (release, debug, kasan, coverage or rust-checks) has the host
# build one from the source (scripts/lib/test-linux-host.sh), a release one if neither is
# set; FXDAX_GUEST_CMDLINE
# adds to its arguments (such as fxfs.follow_apply_delay_us=200). FXDAX_GCE_LIMIT
# caps a run's time on the host (timeout's DURATION, default 6h).
# FXDAX_WORKSPACE_TESTS=0 runs the guest tests alone, FXDAX_GUEST_TESTS=0 the
# workspace's alone, and FXDAX_TEST_ARGS are cargo
# nextest's arguments for the workspace's, such as a filter. FXDAX_EXTENTD_REMOTE,
# if set, names a bucket (gs://bucket/prefix) the guest tests' extent daemons
# keep their stores' branches in, under a prefix of the run's own: the host, made
# with FXDAX_GCE_STORAGE set, reaches it (scripts/lib/gce.sh). FXDAX_RESULTS_BUCKET, if set,
# names one (gs://bucket/prefix) the guest tests' results go to as each case ends, under the run's
# prefix (scripts/guest_results.py), from which they are fetched if the host's are lost: a host
# killed mid-run keeps its finished cases. Every other
# FXDAX_* and PJDFSTEST_* variable set here reaches the host too
# (scripts/lib/gce.sh), such as FXDAX_FOLLOW_SECONDS and FXDAX_PJDFSTEST_TESTS.
set -euo pipefail
[[ ${FXDAX_E2E_TEST:-} =~ ^[a-z0-9\ -]*$ ]] || { echo "FXDAX_E2E_TEST selects test cases" >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
source "$repo/scripts/lib/gce.sh"
zone=${FXDAX_GCE_ZONE:-us-east4-a}
action=${1:-all}
instance=${2:-fxdax-test-$(date -u +%Y%m%d-%H%M%S)-$gce_maker}
results=${3:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce.XXXXXX")}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-test-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-test- instance name.' >&2; exit 2; }
machine=${FXDAX_GCE_MACHINE:-n2-standard-4}
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
    run=$(gce_run_name)
    gce_push_source "$run"
    # The run's own prefixes of the buckets, if they are named.
    local remote= kept=
    if [[ -n ${FXDAX_EXTENTD_REMOTE:-} ]]; then remote=${FXDAX_EXTENTD_REMOTE%/}/$run; fi
    if [[ -n ${FXDAX_RESULTS_BUCKET:-} ]]; then kept=${FXDAX_RESULTS_BUCKET%/}/$run; fi
    # shellcheck disable=SC2086 # The tests' selectors are words separated by spaces.
    gce_remote_detached "$run" "$(gce_host_command "$run" "FXDAX_EXTENTD_REMOTE=$remote" \
        "FXDAX_RESULTS_BUCKET=$kept" test-linux-host.sh ${FXDAX_E2E_TEST:-})" \
        > "$results/remote.log" 2>&1 || status=$?
    finish "$run" "$kept" "$status"
}

# resume RUN: waits for the run RUN on the host, and fetches its results, as `run` does.
resume() {
    local run=$1 status=0 kept=
    [[ "$run" =~ ^run-[0-9TZ]+-[0-9a-f]+$ ]] || { echo 'Name the run, run-...' >&2; exit 2; }
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    if [[ -n ${FXDAX_RESULTS_BUCKET:-} ]]; then kept=${FXDAX_RESULTS_BUCKET%/}/$run; fi
    gce_remote_wait "$run" >> "$results/remote.log" 2>&1 || status=$?
    finish "$run" "$kept" "$status"
}

# finish RUN KEPT STATUS: fetches the run RUN's results, those its bucket KEPT if the host's are
# lost, and says how it went, returning STATUS unless the fetch failed.
finish() {
    local run=$1 kept=$2 status=$3
    gce_fetch "$run" || status=$?
    # The guest tests' results the host didn't give back, as a host lost mid-run doesn't: what
    # the bucket kept of them, each case as it ended.
    if [[ -n $kept && ! -e $results/e2e/summary.json ]]; then
        echo "The host's results are lost: fetching what $kept kept." >&2
        "$repo/scripts/guest_results.py" fetch "$kept/e2e" "$results/e2e" || status=$?
    fi
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
    resume) gce_locate; resume "${4:-}" ;;
    delete) gce_locate; gce_delete ;;
    all) mkdir -p "$results"; gce_all run ;;
    *) echo "Usage: $0 [all|create|run|delete] [instance] [results-directory] | resume instance results run" >&2; exit 2 ;;
esac
