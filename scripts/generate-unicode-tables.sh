#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Regenerate crates/fxfs-unicode-gen/src/lib.rs from the Unicode Character
# Database with Fxfs's own generator. The version must stay 12.1.0: casefolded
# names are hashed into directory keys on disk.
set -euo pipefail

VERSION=12.1.0
SHA256=25ba51a0d4c6fa41047b7a5e5733068d4a734588f055f61e85f450097834a0a6
root="$(cd "$(dirname "$0")/.." && pwd)"
zip="$root/target/UCD-$VERSION.zip"

if [ ! -f "$zip" ]; then
    mkdir -p "$root/target"
    curl -fsSL -o "$zip" "https://www.unicode.org/Public/$VERSION/ucd/UCD.zip"
fi
echo "$SHA256  $zip" | shasum -a 256 -c -

cargo run -q --manifest-path "$root/Cargo.toml" -p fxfs-unicode-gen-builder -- \
    "$zip" "$root/crates/fxfs-unicode-gen/src/lib.rs"
