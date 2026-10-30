#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Print the commit a build of the tree at ROOT (default: this script's) is of, which every build
# reports (TASK-111.7): extentd's /metrics, fxdaxctl --version, the VMM's report to extentd
# (crates/fxdax/build.rs) and the guest kernel's /proc/fs/fxfs/build
# (scripts/build-guest-kernel.sh).
#
#   build-commit.sh [ROOT]
#
# - FXDAX_BUILD_COMMIT, if set, as it is;
# - else, in a git checkout of the tree, its commit, with -dirty if a tracked file differs from it;
# - else source-revision.txt at the tree's root, which the source a GCE run ships carries, as this
#   script printed it where the run was started (scripts/lib/gce.sh): the host's copy has no .git;
# - else `unknown`.
set -euo pipefail

root=$(cd "${1:-$(dirname "$0")/..}" && pwd -P)
if [[ -n ${FXDAX_BUILD_COMMIT:-} ]]; then
    echo "$FXDAX_BUILD_COMMIT"
# The tree's own checkout, not one around it: the host's copy has none, and may lie in one.
elif [[ $(git -C "$root" rev-parse --show-toplevel 2> /dev/null) == "$root" ]]; then
    commit=$(git -C "$root" rev-parse HEAD)
    if [[ -n $(git -C "$root" status --porcelain --untracked-files=no) ]]; then
        commit+=-dirty
    fi
    echo "$commit"
elif [[ -f $root/source-revision.txt ]]; then
    sed -n 1p "$root/source-revision.txt"
else
    echo unknown
fi
