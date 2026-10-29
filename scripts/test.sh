#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run clippy's rules (clippy.toml), the test suite, the concurrency tests under Shuttle, and the
# scripts' tests (scripts/tests).
# Arguments are passed to the suite's `cargo nextest run`.
#
# Unicode tests compare against the Unicode Character Database, which is
# downloaded and checksummed on first use.
set -euo pipefail

VERSION=12.1.0
SHA256=25ba51a0d4c6fa41047b7a5e5733068d4a734588f055f61e85f450097834a0a6
root="$(cd "$(dirname "$0")/.." && pwd)"
zip="$root/target/UCD-$VERSION.zip"

if [ ! -f "$zip" ]; then
    mkdir -p "$root/target"
    curl -fsSL -o "$zip" "https://www.unicode.org/Public/$VERSION/ucd/UCD.zip"
fi
echo "$SHA256  $zip" | shasum -a 256 -c - >/dev/null

# glibc's fatal errors (a heap found corrupted, a stack smashed) on standard error, which the tests
# keep, rather than the terminal they have none of: an abort said nothing else (TASK-100).
export LIBC_FATAL_STDERR_=1

# Clippy's rules the code keeps (clippy.toml): no blocking call in async code unless allowed, with
# why (TASK-44.20). First, as it fails at once.
UCD_ZIP="$zip" cargo clippy --manifest-path "$root/Cargo.toml" --workspace --tests -- \
    -A clippy::all -D clippy::disallowed_methods
UCD_ZIP="$zip" cargo nextest run --manifest-path "$root/Cargo.toml" --workspace "$@"
# The concurrency tests under Shuttle, built with each crate's `shuttle` feature, which only they
# can run (TASK-44).
cargo nextest run --manifest-path "$root/Cargo.toml" -p extent_store --features shuttle \
    --test shuttle
cargo nextest run --manifest-path "$root/Cargo.toml" -p extent_store --features shuttle --lib \
    shuttle_tests::
cargo nextest run --manifest-path "$root/Cargo.toml" -p physmem --features shuttle --lib \
    shuttle::
cargo nextest run --manifest-path "$root/Cargo.toml" -p fxdax --features shuttle --lib \
    shuttle_tests::
# The followers' concurrency protocol, under Shuttle (TASK-42.5).
cargo nextest run --manifest-path "$root/Cargo.toml" -p follower --features shuttle
# The followers' simulation: Fxfs as writer and followers on an extent store (TASK-42.1).
cargo nextest run --manifest-path "$root/Cargo.toml" -p follower_sim --features shuttle
# The scripts' own tests.
python3 -m unittest discover -s "$root/scripts/tests"
