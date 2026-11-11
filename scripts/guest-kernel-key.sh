#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Print a key of what scripts/build-guest-kernel.sh builds the guest kernel from, as the tree ROOT
# (default: this script's) has it: guest/linux (the series and the files it adds), the kernel's Rust
# (guest/rust and every crate of the repository's it builds, as cargo's metadata of its workspace
# says, and the workspace's Cargo.toml they inherit from) and the build's scripts, each file by its
# content: so a tree changed since its commit, as a mutant's (scripts/guest-mutants-host.py), or a
# copy with no .git (host_tree, scripts/lib/linux-host-setup.sh), has the key of what it holds. A
# kernel built from one tree serves another of the same key: CI's sandboxes job keeps one so
# (TASK-330.1), and a host's runs (host_kernel). The kernel says the commit it was built from
# (/proc/fs/fxfs/build), not the one it serves.
#
#   guest-kernel-key.sh [ROOT]
set -euo pipefail

root=$(cd "${1:-$(dirname "$0")/..}" && pwd)
cd "$root"
# The metadata whole first, so that cargo failing fails this. guest/rust's own toolchain's.
metadata=$(env -u RUSTUP_TOOLCHAIN cargo metadata --manifest-path guest/rust/Cargo.toml \
    --format-version 1)
crates=$(python3 -c '
import json, os, sys
root = os.getcwd()
for package in json.load(sys.stdin)["packages"]:
    if package["source"] is None:
        print(os.path.relpath(os.path.dirname(os.path.realpath(package["manifest_path"])), root))
' <<< "$metadata")
[[ -n $crates ]] || { echo "guest/rust's metadata names no crate of the repository's" >&2; exit 1; }
# Each file of them, by its path and content; not what builds leave there.
# shellcheck disable=SC2086 # The crates are paths, a line each.
find Cargo.toml guest/linux guest/rust scripts/build-guest-kernel.sh scripts/build-guest-sysroot.sh \
    $crates \( -name target -o -name '*.o' -o -name '.*.cmd' -o -name '*.a' \) -prune -o \
    -type f -print0 | LC_ALL=C sort -zu | xargs -0 sha256sum | sha256sum | cut -c1-32
