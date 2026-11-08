#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on a kept x86-64 GCE host with nested KVM (scripts/lib/gce.sh),
# reused from run to run, so that each builds only what changed. It runs the
# workspace's tests and fxdax's guest tests (guest/tests), in its Firecracker,
# whose DAX window takes KVM's faults through userfaultfd. `all` acquires a host,
# stopped or idle, and stops it as it ends, whatever happens; `create` holds one
# for a session of `run`s, until its `stop`; `delete` deletes one broken beyond
# repair; `adopt` makes a stopped host the old scripts made a kept one (gce_main). `resume` waits for a run whose waiter here was stopped,
# named as the host names it (RUN, of its ~/RUN.job, and run.txt's in its
# results), and fetches its results as `run` would: on the host, started again if
# it was stopped, which it claims for RUN (an `all` run's, which it then stops) or
# finds held by a `create`.
#
# Usage: test-gce.sh [all|create] [results-directory]
#        test-gce.sh run|stop|delete|adopt instance [results-directory]
#        test-gce.sh resume instance results-directory run
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), FXDAX_GCE_MACHINE the machine type
# (default: n2-standard-4), FXDAX_GCE_RUN the run's number, as its caller counts
# runs (such as r1040), which its name carries (gce_run_name), and FXDAX_E2E_TEST the guest tests' cases to
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
# keep their stores' branches in, under a prefix of the run's own: the host, of
# the kind made with FXDAX_GCE_STORAGE set, reaches it (scripts/lib/gce.sh). FXDAX_RESULTS_BUCKET, if set,
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
results=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce.XXXXXX")
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
machine=${FXDAX_GCE_MACHINE:-n2-standard-4}
limit=${FXDAX_GCE_LIMIT:-6h}
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

# resume RUN: waits for the run RUN on the host, and fetches its results, as `run` does: the host
# started if it is stopped, and claimed for RUN, unless a `create` holds it; and stopped as it ends
# if RUN holds it, as it does an `all` run's.
resume() {
    local run=$1 status=0 kept='' holder
    [[ "$run" =~ $gce_run_pattern ]] || { echo 'Name the run, run-...' >&2; exit 2; }
    gce_check_owner
    mkdir -p "$results"
    results=$(cd "$results" && pwd)
    echo "Results: $results" >&2
    if [[ $(gce_status) == TERMINATED ]]; then
        gce_start
    fi
    gce_wait_ready
    holder=$(gce_holder 2> /dev/null) || holder=
    if [[ $holder != held-* ]]; then
        trap 'status=$?; trap - EXIT; if ! gce_release; then exit 1; fi; exit "$status"' EXIT
        gce_claim "$run" again ||
            { echo "$instance is another run's (${holder:-unknown}), not $run's." >&2; exit 1; }
    fi
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

if [[ ${1:-} == resume ]]; then
    (( $# == 4 )) || { echo "Usage: $0 resume instance results-directory run" >&2; exit 2; }
    [[ $2 =~ ^fxdax-[a-z0-9-]+$ ]] || { echo 'Name a host, fxdax-...' >&2; exit 2; }
    instance=$2 results=$3
    gce_locate
    resume "$4"
else
    gce_main "$@"
fi
