#!/usr/bin/env bash
# Qualify fxdax on a disposable x86-64 GCE host with nested KVM
# (scripts/lib/gce.sh). It runs the workspace's tests and libkrun's virtio-fs
# guest tests, whose DAX window takes KVM's faults through userfaultfd. `all`
# always deletes its host; `create`, `run` and `delete` expose the same steps
# for interrupted runs.
#
# Usage: test-gce.sh [all|create|run|delete] [instance] [results-directory]
#
# FXDAX_GCE_PROJECT is the project (default: gcloud's), FXDAX_GCE_ZONE the zone
# to try first (default: us-east4-a), and FXDAX_E2E_TEST a libkrun test case to
# run instead of scripts/test-e2e.sh's (`all` runs every one).
set -euo pipefail
[[ ${FXDAX_E2E_TEST:-} =~ ^[a-z0-9-]*$ ]] || { echo "FXDAX_E2E_TEST is a test case name" >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
action=${1:-all}
instance=${2:-fxdax-test-$(date -u +%Y%m%d-%H%M%S)}
results=${3:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-gce.XXXXXX")}
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$instance" =~ ^fxdax-test-[a-z0-9-]+$ ]] || { echo 'Use an fxdax-test- instance name.' >&2; exit 2; }
machine=n2-standard-8
purpose=fxdax-test
# The host deletes itself after this long; the run's own limit leaves time to
# copy the results back.
lifetime=8h limit=6h
create_flags=(--min-cpu-platform='Intel Cascade Lake')
source "$repo/scripts/lib/gce.sh"

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
    gce_remote "sudo timeout --signal=TERM --kill-after=30s $limit \
        bash $run/source/scripts/lib/test-linux-host.sh $run/source $run/results ${FXDAX_E2E_TEST:-}" \
        > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    tail -n 4 "$results/remote.log" >&2
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
