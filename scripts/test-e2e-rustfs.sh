#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run scripts/test-e2e.sh's cases, as it selects them by its arguments, with their extent daemons'
# branches in an S3 bucket on this machine, as production keeps them in object storage: RustFS
# (github.com/rustfs/rustfs, Apache-2.0), as scripts/lib/rustfs.sh starts it, a process,
# its bucket s3://fxdax/e2e (FXDAX_EXTENTD_REMOTE), its address and credentials in AWS_*, which
# the cases' own daemons find it by too; stopped at the end. Its objects are in FXDAX_RUSTFS_DATA,
# a directory, if set, else in one of its own. What test-e2e.sh takes from the environment
# passes through. The nightly's guest suites run so on GitHub's runners (TASK-284), and
# scripts/sandboxes.sh's (TASK-330.1).
#
# Exits test-e2e.sh's status, or 2 if the server couldn't be started.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/lib/rustfs.sh"
rustfs_start "${FXDAX_RUSTFS_DATA:-}" || exit 2
trap rustfs_stop EXIT
status=0
FXDAX_EXTENTD_REMOTE=s3://fxdax/e2e "$root/scripts/test-e2e.sh" "$@" || status=$?
exit "$status"
