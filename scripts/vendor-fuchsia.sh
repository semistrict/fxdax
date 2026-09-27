#!/usr/bin/env bash
# Copy the Fuchsia sources fxdax builds on into vendor/fuchsia, unmodified.
#
# Usage: scripts/vendor-fuchsia.sh [FUCHSIA_CHECKOUT]
#
# The checkout (default ~/src/fuchsia) must be at $COMMIT; paths missing from
# a sparse checkout are added to it. Local changes to vendored files are
# committed separately, so re-running this and diffing shows them.
set -euo pipefail

COMMIT=90e54e090540aaac37a81494884f4b8a68bc269c
FUCHSIA="${1:-$HOME/src/fuchsia}"
DEST="$(cd "$(dirname "$0")/.." && pwd)/vendor/fuchsia"

PATHS=(
    # Fxfs
    src/storage/fxfs
    # Runtime
    src/lib/fuchsia
    src/lib/fuchsia-async
    src/lib/fuchsia-async-macro
    src/lib/fuchsia-emulated-handle
    src/lib/fuchsia-sync
    sdk/rust/zx-status
    sdk/rust/zx-status-ext
    sdk/rust/zx-types
    # Storage libraries
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
    # Interface definitions the stand-in crates mirror
    sdk/fidl/fuchsia.io
    # Versions of third-party crates Fuchsia builds with
    third_party/rust_crates/Cargo.toml
)

head="$(git -C "$FUCHSIA" rev-parse HEAD)"
if [ "$head" != "$COMMIT" ]; then
    echo "error: $FUCHSIA is at $head, expected $COMMIT" >&2
    exit 1
fi

if [ "$(git -C "$FUCHSIA" config --bool core.sparseCheckout || true)" = true ]; then
    git -C "$FUCHSIA" sparse-checkout add --skip-checks "${PATHS[@]}"
fi

mkdir -p "$DEST"
for path in "${PATHS[@]}"; do
    mkdir -p "$DEST/$(dirname "$path")"
    rsync -a --delete "$FUCHSIA/$path" "$DEST/$(dirname "$path")/"
done
cp "$FUCHSIA/LICENSE" "$FUCHSIA/PATENTS" "$FUCHSIA/AUTHORS" "$DEST/"
echo "$COMMIT" > "$DEST/COMMIT"
