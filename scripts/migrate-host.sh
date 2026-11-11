#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Move a VM between hosts through a bucket (TASK-28.11), on Linux hosts of one's own, over ssh
# (scripts/lib/host.sh). On the first, the guest test fxfs-guest-migrate-dax runs a VM whose store's
# branch its extent daemon keeps in the bucket, and snapshots it, the snapshot, its memory and the
# VM's root filesystem left in a bundle; then the run there ends, its directory gone; on the
# second, which has nothing of the store, the same test restores the VM from the bundle, its store
# forked from the snapshot's point, whose branch the daemon takes from the bucket, and the guest
# checks what it wrote before, and writes and reads more (test_fxfs_guest_snapshot.rs). Both run
# the commit from the same paths, as the restored VM goes on reading its root filesystem where it
# was. The hosts' results go to RESULTS/a and RESULTS/b; the bundle, and the bucket if it is the
# run's own, go as the run ends, whatever happens.
#
#   scripts/migrate-host.sh [RESULTS]
#
# FXDAX_MIGRATE_HOSTS names the two hosts, as ssh does (default: FXDAX_HOST twice, ubu1's, the
# second run on the same host as the first, after it): of one CPU model, which a snapshot restores
# on. FXDAX_MIGRATE_BUCKET names the bucket and prefix the run keeps its branches under, an object
# store's URL both hosts reach, under a prefix of the run's own, which the run leaves; by default,
# with one host for both, a directory of the run's own there (file://). FXDAX_GUEST_KERNEL names
# fxdax's x86-64 guest kernel here, or FXDAX_GUEST_KERNEL_BUILD the one each host builds (release by
# default, scripts/test-host.sh). Every other FXDAX_* and PJDFSTEST_* variable set here reaches
# the hosts too (scripts/lib/host.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$repo/scripts/lib/host.sh"
limit=2h
read -ra hosts <<< "${FXDAX_MIGRATE_HOSTS:-$host $host}"
(( ${#hosts[@]} == 2 )) || { echo "FXDAX_MIGRATE_HOSTS names two hosts" >&2; exit 2; }
for each in "${hosts[@]}"; do
    [[ $each =~ ^[A-Za-z0-9._@-]+$ ]] || { echo "FXDAX_MIGRATE_HOSTS names hosts, as ssh does: $each" >&2; exit 2; }
done
top=${1:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-migrate.XXXXXX")}
mkdir -p "$top"
top=$(cd "$top" && pwd)
# What outlasts each host's run: the bundle, and the bucket if it is the run's own.
held=$host_scratch/$host_run.held
if [[ -n ${FXDAX_MIGRATE_BUCKET:-} ]]; then
    [[ $FXDAX_MIGRATE_BUCKET =~ ^[a-z0-9]+://[A-Za-z0-9._/-]*$ ]] ||
        { echo "FXDAX_MIGRATE_BUCKET is an object store's URL, as s3://bucket/prefix" >&2; exit 2; }
    remote=${FXDAX_MIGRATE_BUCKET%/}/$host_run
elif [[ ${hosts[0]} == "${hosts[1]}" ]]; then
    remote=file://$held/bucket
else
    echo "Name a bucket both hosts reach: FXDAX_MIGRATE_BUCKET." >&2
    exit 2
fi

# cleanup, the trap, ends the run on the host it is on, and removes what it held on each host.
cleanup() {
    local status=$? each
    trap - EXIT
    host_end || status=1
    for each in "${hosts[@]}"; do
        host=$each
        host_ssh "if [ -e $(printf %q "$held") ]; then sudo rm -r -- $(printf %q "$held"); fi" ||
            status=1
    done
    echo "Results: $top" >&2
    exit "$status"
}
trap cleanup EXIT

# migrate SIDE PHASE TOKEN runs the test's PHASE, alone, on the host of SIDE (0 for a, 1 for b), as
# of the placement TOKEN, its branches in the bucket, its results in RESULTS/a or b; and ends the
# run there.
migrate() {
    local side=$1 phase=$2 token=$3 status=0
    host=${hosts[$side]} results=$top/$(tr 01 ab <<< "$side")
    # host_start's trap is this script's.
    host_start || status=$?
    trap cleanup EXIT
    (( status == 0 )) || return "$status"
    host_ssh "mkdir -p $(printf %q "$held")" || return
    host_job FXDAX_WORKSPACE_TESTS=0 "FXDAX_EXTENTD_REMOTE=$remote/branches" \
        "FXDAX_EXTENTD_TOKEN=$token" "FXDAX_MIGRATE_PHASE=$phase" "FXDAX_MIGRATE_BUNDLE=$held/bundle" \
        test-linux-host.sh fxfs-guest-migrate-dax > "$results/remote.log" 2>&1 || status=$?
    if [[ $host_state == ended ]]; then
        host_fetch || status=$?
    fi
    sed -n '$p' "$results/remote.log" >&2
    host_end || status=1
    return "$status"
}

migrate 0 source 1
if [[ ${hosts[0]} != "${hosts[1]}" ]]; then
    # The bundle, from the first host to the second.
    host=${hosts[0]}
    host_ssh "sudo tar -C $(printf %q "$held") -czf - bundle" |
        ssh "${host_ssh_options[@]}" "${hosts[1]}" \
            "mkdir -p $(printf %q "$held") && tar -C $(printf %q "$held") -xzf -"
fi
# A later placement than the first host's.
migrate 1 target 2
echo "The VM moved: restored on ${hosts[1]} from what ${hosts[0]} left in $remote." >&2
