#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Refuses a vendor/firecracker that moves back or sideways between two revisions of this repo:
# from OLD's commit of the fork, NEW's must be ahead, as GitHub's compare says, or the same.
# b781ff302 moved main's back to before VIRTIO_EXTENT_F_SEAL (a2db2b3b0), a submodule left behind
# in a worktree committed with the rest, and nothing noticed: CI builds no Firecracker, and the
# guest suites pass without the daemon's asks to seal (TASK-166).
#
# Usage: firecracker-forward.sh OLD NEW
# FXDAX_FIRECRACKER_REPO names the fork on GitHub (default semistrict/firecracker).
set -euo pipefail

[[ $# == 2 ]] || { echo "Usage: $0 OLD NEW" >&2; exit 2; }
repo=${FXDAX_FIRECRACKER_REPO:-semistrict/firecracker}
old=$(git rev-parse "$1:vendor/firecracker")
new=$(git rev-parse "$2:vendor/firecracker")
[[ $old == "$new" ]] && exit 0
status=$(gh api "repos/$repo/compare/$old...$new" --jq .status)
if [[ $status != ahead ]]; then
    echo "vendor/firecracker goes from ${old:0:12} to ${new:0:12}, $status of it: not forward" >&2
    exit 1
fi
