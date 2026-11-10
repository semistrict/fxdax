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
# short (the default) is for every push, minutes; long, eight VMs at once of 32, for nightly. The disk is FXDAX_SANDBOX_DISK's
# (default: TMPDIR's), which must be ext4: the run's directory there holds the bucket's data, which
# RustFS (github.com/rustfs/rustfs, Apache-2.0), in a container (docker or podman,
# scripts/lib/containers.sh), serves, and scripts/test-e2e.sh's results (FXDAX_E2E_RESULTS, by
# default in the run's directory), the case's stores and the daemons' part caches among them. What
# test-e2e.sh takes from the environment passes through (FXDAX_GUEST_KERNEL, FXDAX_E2E_PREBUILT).
# The bucket's data goes at the end, and the run's directory with it unless it holds the results.
#
# Exits 0 if the run passed, 1 if it failed (test-e2e.sh's status), 2 if it couldn't be set up.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
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

image="${FXDAX_RUSTFS_IMAGE:-rustfs/rustfs:1.0.4}"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
# Test credentials, for this server alone.
export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true
source "$root/scripts/lib/containers.sh"
engine="$(container_engine)" || exit 2
# Its objects on the disk, as LangSmith's RustFS kept them beside the hosts' stores, as this user's,
# its container run as this user. Ready once its readiness endpoint says so (scripts/test-rustfs.sh).
server="$(healthy_container "$engine" 'curl -fsS -o /dev/null http://127.0.0.1:9000/health/ready' \
    -p "127.0.0.1:$port:9000" --user "$(id -u):$(id -g)" -v "$bucket_data:/data" \
    -e RUSTFS_VOLUMES=/data \
    -e RUSTFS_ACCESS_KEY="$AWS_ACCESS_KEY_ID" -e RUSTFS_SECRET_KEY="$AWS_SECRET_ACCESS_KEY" \
    "$image")" || exit 2
cleanup() {
    "$engine" stop "$server" > /dev/null || true
    rm -r "$bucket_data"
    # The run's directory, and what a case that failed left in it, its daemons' sockets among
    # them, unless it holds the results.
    [[ $results == "$work"/* ]] || rm -r "$work"
}
trap cleanup EXIT
curl -fsS -X PUT --aws-sigv4 "aws:amz:$AWS_REGION:s3" \
    --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$AWS_ENDPOINT/fxdax" || exit 2

status=0
TMPDIR="$work" FXDAX_E2E_RESULTS="$results" FXDAX_EXTENTD_REMOTE=s3://fxdax/sandboxes \
    FXDAX_SANDBOX_BUCKET_DIR="$bucket_data" FXDAX_SANDBOX_MODE="$mode" \
    "$root/scripts/test-e2e.sh" fxfs-guest-sandboxes || status=$?
# What the case said of its run, and each check it failed, from its log: the runner says only
# that it failed.
find "$results" -name log.txt -path '*fxfs-guest-sandboxes*' -exec grep -h '^sandboxes: ' {} + >&2 ||
    true
echo "sandboxes ($mode): exit $status, results in $results" >&2
exit "$status"
