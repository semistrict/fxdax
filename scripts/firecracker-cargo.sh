#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs cargo in Firecracker as fxdax builds it: vendor/firecracker, whose `fxdax` feature builds
# the extent device against fxdax's crate, which the series names nowhere, so that it builds
# outside this checkout as upstream's does (TASK-269, its resources/fxdax/README.md). fxdax's build
# supplies the rest, in a copy of the tree, target/firecracker, which this keeps and builds in:
#
# - fxdax's crate, a dependency appended to the copy's src/vmm/Cargo.toml;
# - fxdax's lock, vendor/firecracker.lock, the copy's Cargo.lock: the series' own has none of
#   fxdax's dependencies. Cargo must find it as it is (as --locked does), unless
#   FXDAX_FIRECRACKER_UNLOCKED is set;
# - frame pointers, from this repo's .cargo/config.toml, which Cargo reads in the copy as in any
#   directory below it (TASK-92.39).
#
# Callers pass the feature, as `--features fxdax`, where they build the extent device. The copy is
# by content, as scripts/lib/linux-host-setup.sh's host_tree is, writing only what differs, so
# that what changed is newer than what was built from it. Its build is in its build/cargo_target,
# as Firecracker's .cargo/config.toml sets.
#
# Usage: firecracker-cargo.sh CARGO_ARGS...   e.g. build --release -p firecracker --features fxdax
#        firecracker-cargo.sh --lock          updates vendor/firecracker.lock as Cargo resolves it
#        firecracker-cargo.sh --tree          prints the copy's directory
# FXDAX_FIRECRACKER_SRC names the tree to copy (default vendor/firecracker), and
# FXDAX_FIRECRACKER_TREE the copy (default target/firecracker), which must be in this repo.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src=${FXDAX_FIRECRACKER_SRC:-$root/vendor/firecracker}
tree=${FXDAX_FIRECRACKER_TREE:-$root/target/firecracker}
lock=$root/vendor/firecracker.lock

[[ $# -gt 0 ]] || { echo "Usage: $0 CARGO_ARGS... | --lock | --tree" >&2; exit 2; }
[[ -f $src/src/vmm/Cargo.toml ]] || { echo "$src: no Firecracker (git submodule update --init vendor/firecracker)" >&2; exit 1; }
case "$tree/" in
    "$root"/*) ;;
    *) echo "FXDAX_FIRECRACKER_TREE $tree is not in $root, whose .cargo/config.toml it needs" >&2; exit 2 ;;
esac

# put FILE TARGET writes FILE's content to TARGET if it differs: cargo rebuilds what a newer
# manifest is of.
put() {
    cmp -s "$1" "$2" || cp "$1" "$2"
}

mkdir -p "$tree"
rsync -rlpD --checksum --delete --exclude /.git --exclude /build --exclude /Cargo.lock \
    --exclude /src/vmm/Cargo.toml "$src/" "$tree/"
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
{
    cat "$src/src/vmm/Cargo.toml"
    printf '\n# fxdax: its crate, which the fxdax feature builds against (scripts/firecracker-cargo.sh).\n'
    printf '[dependencies.fxdax]\npath = "%s"\n' "$root/crates/fxdax"
} > "$manifest"
put "$manifest" "$tree/src/vmm/Cargo.toml"
put "$lock" "$tree/Cargo.lock"
cd "$tree"

case "$1" in
    --tree)
        echo "$tree"
        ;;
    --lock)
        cargo metadata --format-version 1 > /dev/null
        put Cargo.lock "$lock"
        ;;
    *)
        if [[ -z ${FXDAX_FIRECRACKER_UNLOCKED:-} ]] &&
            ! cargo metadata --locked --format-version 1 > /dev/null; then
            echo "vendor/firecracker.lock is not as Cargo resolves the tree: scripts/firecracker-cargo.sh --lock updates it" >&2
            exit 1
        fi
        exec cargo "$@"
        ;;
esac
