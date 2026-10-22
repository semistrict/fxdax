#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Move a VM between two disposable GCE hosts through GCS (TASK-28.11). On the
# first, the guest test fxfs-guest-migrate-dax runs a VM whose store's branch
# its extent daemon keeps in the bucket, snapshots it, and the snapshot, its
# memory and the VM's root filesystem go to the bucket too; on the second,
# which has nothing of the store, the same test restores the VM from there, its
# store forked from the snapshot's point, whose branch the daemon takes from the
# bucket, and the guest checks what it wrote before, and writes and reads more
# (test_fxfs_guest_snapshot.rs). Both hosts are of one machine type with one CPU
# model, as a snapshot restores on, and run the source from the same paths, as
# the restored VM goes on reading its root filesystem where it was. Both hosts,
# and what the run put in the bucket, are deleted whatever happens.
#
#   scripts/migrate-gce.sh [results-directory]
#
# FXDAX_MIGRATE_BUCKET names the bucket and prefix (gs://bucket/prefix) the run
# keeps its branches and the snapshot under, FXDAX_GUEST_KERNEL fxdax's x86-64
# guest kernel (scripts/build-guest-kernel.sh); FXDAX_GCE_PROJECT is the project
# (default: gcloud's), FXDAX_GCE_ZONE the zone to try first (default:
# us-east4-a, the bucket's region). Every other FXDAX_* and PJDFSTEST_*
# variable set here reaches the hosts too (scripts/lib/gce.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
project=${FXDAX_GCE_PROJECT:-$(gcloud config get-value project 2>/dev/null)}
zone=${FXDAX_GCE_ZONE:-us-east4-a}
bucket=${FXDAX_MIGRATE_BUCKET:?FXDAX_MIGRATE_BUCKET names the bucket: gs://bucket/prefix}
: "${FXDAX_GUEST_KERNEL:?FXDAX_GUEST_KERNEL names the x86-64 guest kernel}"
[[ -n "$project" && "$project" != '(unset)' ]] || { echo 'Set FXDAX_GCE_PROJECT.' >&2; exit 2; }
[[ "$bucket" =~ ^gs://[a-z0-9._-]+(/[A-Za-z0-9._/-]*)?$ ]] || { echo 'FXDAX_MIGRATE_BUCKET is gs://bucket/prefix' >&2; exit 2; }
stamp=$(date -u +%Y%m%d-%H%M%S)
top=${1:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-migrate.XXXXXX")}
mkdir -p "$top"
top=$(cd "$top" && pwd)
# One CPU model (Sapphire Rapids), which a snapshot restores on, with nested KVM.
machine=c3-standard-8
purpose=fxdax-test
lifetime=8h limit=2h
create_flags=()
# The hosts read and write the bucket with the project's default service account.
FXDAX_GCE_STORAGE=1
results=$top
source "$repo/scripts/lib/gce.sh"
default_zone=$zone
hosts=("fxdax-test-migrate-a-$stamp" "fxdax-test-migrate-b-$stamp")
# The same on both hosts: the test's paths, the root filesystem's among them, are too.
run="run-$stamp"
remote="$bucket/$run"

# on HOST points gce.sh's functions at HOST, and at its directory of results.
on() {
    instance=$1
    results="$top/$1"
    mkdir -p "$results"
    zone=$default_zone
    gce_locate
}

cleanup() {
    local status=$? host
    trap - EXIT
    for host in "${hosts[@]}"; do
        on "$host"
        gce_delete || status=1
    done
    gcloud storage rm --recursive --quiet "$remote" > /dev/null 2>&1 || true
    echo "Results: $top" >&2
    exit "$status"
}
trap cleanup EXIT

for host in "${hosts[@]}"; do
    on "$host"
    gce_create
done
for host in "${hosts[@]}"; do
    on "$host"
    gce_check_owner
    gce_wait_ready
    gce_push_source "$run"
done

# gcloud on the host, to reach the bucket.
gcloud_ready='command -v gcloud > /dev/null || sudo snap install google-cloud-cli --classic > /dev/null'

# migrate PHASE TOKEN runs the test's PHASE, alone, on the host, as of the placement TOKEN, its
# branches in the bucket.
migrate() {
    local phase=$1 token=$2 status=0
    # shellcheck disable=SC2088 # The host's home (gce_host_command).
    gce_remote "$(gce_host_command "$run" FXDAX_WORKSPACE_TESTS=0 \
        "FXDAX_EXTENTD_REMOTE=$remote/branches" "FXDAX_EXTENTD_TOKEN=$token" \
        "FXDAX_MIGRATE_PHASE=$phase" "FXDAX_MIGRATE_BUNDLE=~/$run/bundle" \
        test-linux-host.sh fxfs-guest-migrate-dax)" > "$results/remote.log" 2>&1 || status=$?
    gce_fetch "$run" || status=$?
    sed -n '$p' "$results/remote.log" >&2
    return "$status"
}

on "${hosts[0]}"
migrate source 1
gce_remote "$gcloud_ready; sudo gcloud storage cp --recursive \$HOME/$run/bundle $remote/" \
    >> "$results/remote.log" 2>&1
on "${hosts[1]}"
gce_remote "$gcloud_ready; sudo gcloud storage cp --recursive $remote/bundle \$HOME/$run/" \
    > "$results/bundle.log" 2>&1
# A later placement than the first host's.
migrate target 2
echo "The VM moved: restored on ${hosts[1]} from what ${hosts[0]} left in $remote." >&2
