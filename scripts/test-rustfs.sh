#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run the branches' tests against RustFS, an S3 server (github.com/rustfs/rustfs, Apache-2.0), on
# this machine, as scripts/lib/rustfs.sh starts it, a process, its bucket made: run the S3
# tests (ignored in the workspace's run), collect what they left, and stop it. CI's object-stores
# job runs it (TASK-149.20). Arguments are passed to cargo nextest.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/lib/rustfs.sh"
rustfs_start
trap rustfs_stop EXIT
FXDAX_S3_URL=s3://fxdax/test cargo nextest run --manifest-path "$root/Cargo.toml" \
    -p extent_remote -p fxdax --run-ignored only -E 'test(/rustfs/)' "$@"
# The collector, over what the tests left.
cargo run --quiet --manifest-path "$root/Cargo.toml" -p extent_remote --bin fxdax-gc -- \
    s3://fxdax/test --grace 0
