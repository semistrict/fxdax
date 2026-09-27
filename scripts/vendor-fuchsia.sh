#!/usr/bin/env bash
# Copy the Fuchsia sources fxdax builds on into vendor/fuchsia, unmodified.
#
# Usage: scripts/vendor-fuchsia.sh [REVISION]
#
# REVISION defaults to the commit recorded in vendor/fuchsia/COMMIT, which
# re-vendors the current sources. Any other revision, such as origin/main,
# updates to it: the checkout is fetched, and afterwards
# scripts/upstream-changes.py lists the ported code whose upstream sources
# changed.
#
# The Fuchsia checkout ($FUCHSIA, default ~/src/fuchsia) must be clean. It is
# moved to the revision, and the vendored paths are added to it if it is
# sparse. Local changes to vendored files are committed separately, so
# re-running this and diffing shows them.
set -euo pipefail

FUCHSIA="${FUCHSIA:-$HOME/src/fuchsia}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/fuchsia"
REVISION="${1:-$(cat "$DEST/COMMIT")}"

PATHS=(
    # Fxfs
    src/storage/fxfs
    # Runtime
    sdk/lib/c/include/gen/zircon/process.h
    src/lib/fuchsia
    src/lib/fuchsia-async
    src/lib/fuchsia-async-macro
    src/lib/fuchsia-emulated-handle
    src/lib/fuchsia-sync
    sdk/rust/zx-status
    sdk/rust/zx-status-ext
    sdk/rust/zx-types
    # Storage libraries
    src/storage/crypt/fxfs/common
    src/storage/lib/blob_metadata
    src/storage/lib/block_protocol
    src/storage/lib/fscrypt
    src/storage/lib/fsverity-merkle
    src/storage/lib/ptr_slice
    src/storage/lib/storage_device
    src/storage/lib/storage_xts
    src/storage/lib/trace
    src/storage/lib/units
    src/storage/type-fprint
    src/sys/pkg/lib/fuchsia-hash
    src/sys/pkg/lib/fuchsia-merkle
    # Zircon: the kernel's VM and objects are ported from these, the syscall
    # interface is defined by zx and zx-sys, and the core tests are the spec
    zircon/LICENSE
    zircon/kernel/LICENSE
    zircon/kernel/arch/arm64/exceptions_c.cc
    zircon/kernel/arch/arm64/mmu.cc
    zircon/kernel/arch/riscv64/src/user_copy.rs
    zircon/kernel/arch/x86/exceptions_c.cc
    zircon/kernel/hypervisor/aspace.cc
    zircon/kernel/hypervisor/hypervisor_unittest.cc
    zircon/kernel/hypervisor/include/hypervisor/aspace.h
    zircon/kernel/include
    zircon/kernel/kernel/deadline.rs
    zircon/kernel/kernel/event.rs
    zircon/kernel/kernel/thread.cc
    zircon/kernel/kernel/timer.cc
    zircon/kernel/kernel/types.rs
    zircon/kernel/lib/page
    zircon/kernel/lib/syscalls
    zircon/kernel/lib/unittest/user_memory.rs
    zircon/kernel/lib/user_copy
    zircon/kernel/object
    zircon/kernel/vm
    zircon/system/public/zircon
    zircon/system/utest/core/pager
    zircon/system/utest/core/pager-writeback
    zircon/system/utest/core/vmo
    zircon/vdso
    sdk/rust/zx
    sdk/rust/zx-sys
    # Interface definitions the stand-in crates mirror
    sdk/fidl/fuchsia.diagnostics.types
    sdk/fidl/fuchsia.io
    # Versions of third-party crates Fuchsia builds with
    third_party/rust_crates/Cargo.toml
)

git() { command git -C "$FUCHSIA" "$@"; }

if [ -n "$(git status --porcelain)" ]; then
    echo "error: $FUCHSIA has local changes" >&2
    exit 1
fi

# Fetch unless the revision is a commit that is already present
if ! [[ "$REVISION" =~ ^[0-9a-f]{40}$ ]] || ! git cat-file -e "$REVISION^{commit}" 2>/dev/null; then
    git fetch --quiet origin
fi
commit="$(git rev-parse --verify "$REVISION^{commit}")"
old="$(cat "$DEST/COMMIT")"

if [ "$(git config --bool core.sparseCheckout || true)" = true ]; then
    git sparse-checkout add --skip-checks "${PATHS[@]}"
fi
git switch --quiet --detach "$commit"

mkdir -p "$DEST"
for path in "${PATHS[@]}"; do
    mkdir -p "$DEST/$(dirname "$path")"
    rsync -a --delete "$FUCHSIA/$path" "$DEST/$(dirname "$path")/"
done
cp "$FUCHSIA/LICENSE" "$FUCHSIA/PATENTS" "$FUCHSIA/AUTHORS" "$DEST/"
echo "$commit" > "$DEST/COMMIT"

if [ "$commit" != "$old" ]; then
    count="$(git rev-list --count "$old..$commit" -- "${PATHS[@]}")"
    echo "Updated from $old to $commit; $count upstream commits touch vendored paths."
    "$ROOT/scripts/upstream-changes.py"
fi
