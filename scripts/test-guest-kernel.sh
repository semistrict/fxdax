#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build the guest kernel with Fxfs in it (scripts/build-guest-kernel.sh) and run fxdax's guest
# tests of it (guest/tests) on this host, Linux with KVM, in fxdax's Firecracker: fxfs-guest,
# fxfs-guest-format, fxfs-guest-crash and fxfs-guest-resize, fxfs-guest-live-fork (a running VM's
# store forked, its filesystem frozen), fxfs-guest-follow (a VM following a store another VM
# writes, read-only, as it writes it), fxfs-guest-follow-torture (two followers' readers checking
# what they read as the writer changes it, and the mutant whose followers invalidate nothing,
# which they must catch), fxfs-guest-import (a guest booted on a fork of a store
# made from a container image), fxfs-guest-snapshot (a VM snapshotted and restored twice, each
# into a store of its own; and suspended, and moved through a bucket), fxfs-guest-checks (checks
# of the guest's Fxfs against the host's) and extent-guest (the extent-store device's driver), each with
# a DAX window too where it has a case of it, and pjdfstest-fxfs-guest(-dax) (pjdfstest, whose
# rootfs podman builds). On macOS it runs in Lima (scripts/lib/lima.sh). The kernel is kept in
# $FXDAX_E2E_RESULTS (default: a new temporary directory), with the tests' logs.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
if [[ $(uname -s) == Darwin ]]; then
    exec "$root/scripts/lib/lima.sh" test-guest-kernel.sh "$@"
fi
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)
built=$("$root/scripts/build-guest-kernel.sh" | tail -1)
cp "$built" "$results/"

# The pjdfstest (BSD-2-Clause) commit pjdfstest-fxfs-guest builds.
export PJDFSTEST_REPO=${PJDFSTEST_REPO:-https://github.com/pjd/pjdfstest}
export PJDFSTEST_COMMIT=${PJDFSTEST_COMMIT:-85a8aea9e685999ef0540392fd80535f873d7ff7}
FXDAX_GUEST_KERNEL="$results/$(basename "$built")" FXDAX_E2E_RESULTS="$results" \
    "$root/scripts/test-e2e.sh" extent-guest fxfs-guest fxfs-guest-dax fxfs-guest-format \
    fxfs-guest-crash fxfs-guest-crash-dax fxfs-guest-resize fxfs-guest-live-fork \
    fxfs-guest-follow fxfs-guest-follow-dax fxfs-guest-follow-torture \
    fxfs-guest-follow-torture-dax fxfs-guest-follow-torture-mutant-dax fxfs-guest-import \
    fxfs-guest-import-dax fxfs-guest-snapshot fxfs-guest-snapshot-dax fxfs-guest-suspend-dax fxfs-guest-migrate-dax fxfs-guest-checks \
    fxfs-guest-checks-dax \
    pjdfstest-fxfs-guest pjdfstest-fxfs-guest-dax
