#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Print a key of what scripts/build-guest-kernel.sh builds the guest kernel from, as this checkout's
# commit has it: guest/linux (the series and the files it adds), the kernel's Rust (guest/rust and
# every crate of the repository's it builds, as cargo's metadata of its workspace says) and the
# build's scripts, each by its tree's or blob's git hash. A kernel built from one commit serves
# another of the same key: CI's sandboxes job keeps one so (TASK-330.1). The kernel says the commit
# it was built from (/proc/fs/fxfs/build), not the one it serves.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
# The metadata whole first, so that cargo failing fails this.
metadata=$(cargo metadata --manifest-path guest/rust/Cargo.toml --format-version 1)
mapfile -t crates < <(python3 -c '
import json, os, sys
root = os.getcwd()
for package in json.load(sys.stdin)["packages"]:
    if package["source"] is None:
        print(os.path.relpath(os.path.dirname(package["manifest_path"]), root))
' <<< "$metadata")
(( ${#crates[@]} > 0 )) || { echo "guest/rust's metadata names no crate of the repository's" >&2; exit 1; }
paths=(guest/linux guest/rust scripts/build-guest-kernel.sh scripts/build-guest-sysroot.sh
    "${crates[@]}")
for path in $(printf '%s\n' "${paths[@]}" | sort -u); do
    echo "$path $(git rev-parse "HEAD:$path")"
done | sha256sum | cut -c1-32
