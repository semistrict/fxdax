#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build the guest kernel with Fxfs in it (scripts/build-guest-kernel.sh) and run fxdax's guest
# tests of it (guest/tests) on this host, Linux with KVM, in fxdax's Firecracker: the kernel suite
# (`runner list --tag kernel`), the guest's Fxfs as its every feature is used, with a DAX window
# and without: formats, crashes, resizes, live forks, volumes side by side and shared, a lost
# daemon, followers and their torture tests' mutants, imports, snapshots, suspends and
# migrations, the checks, the extent-store device's driver, and pjdfstest, whose rootfs podman
# builds. On macOS it runs in Lima (scripts/lib/lima.sh). The kernel is kept in
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
    "$root/scripts/test-e2e.sh" --tag kernel
