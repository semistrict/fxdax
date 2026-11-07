#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run the branches' tests against RustFS, an S3 server (github.com/rustfs/rustfs, Apache-2.0), on
# this machine: start it in a container (docker, or podman: scripts/lib/containers.sh), or from its
# binary, create a bucket, run the S3 tests (ignored in the workspace's run), collect what they
# left, and stop it. CI's object-stores job runs it (TASK-149.20). Arguments are passed to cargo
# nextest.
#
# FXDAX_RUSTFS names a RustFS binary to run instead, built from its sources with
# `cargo build --release --bin rustfs`; FXDAX_RUSTFS_IMAGE its image (by default rustfs/rustfs, at
# the version tested).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="${FXDAX_RUSTFS_IMAGE:-rustfs/rustfs:1.0.4}"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
# Test credentials, for this server alone.
export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true
if [[ -n ${FXDAX_RUSTFS:-} ]]; then
    # The server's data and log, under target/, which a later run replaces.
    data="$root/target/rustfs"
    mkdir -p "$data/volume-$port"
    "$FXDAX_RUSTFS" server --address "127.0.0.1:$port" \
        --access-key "$AWS_ACCESS_KEY_ID" --secret-key "$AWS_SECRET_ACCESS_KEY" \
        "$data/volume-$port" >"$data/server-$port.log" 2>&1 &
    server=$!
    trap 'kill "$server"; wait "$server" 2>/dev/null || true' EXIT
else
    source "$root/scripts/lib/containers.sh"
    engine="$(container_engine)"
    server="$("$engine" run -d --rm -p "127.0.0.1:$port:9000" \
        -e RUSTFS_ACCESS_KEY="$AWS_ACCESS_KEY_ID" -e RUSTFS_SECRET_KEY="$AWS_SECRET_ACCESS_KEY" \
        "$image")"
    trap '"$engine" stop "$server" >/dev/null' EXIT
fi
# Up once it answers anything, an unsigned request refused.
curl -s --retry 60 --retry-all-errors --retry-delay 1 -o /dev/null "$AWS_ENDPOINT/"
# The bucket, made with a request curl signs as S3's API wants (SigV4).
curl -sf -X PUT --aws-sigv4 "aws:amz:$AWS_REGION:s3" \
    --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$AWS_ENDPOINT/fxdax"
FXDAX_S3_URL=s3://fxdax/test cargo nextest run --manifest-path "$root/Cargo.toml" \
    -p extent_remote -p fxdax --run-ignored only -E 'test(/rustfs/)' "$@"
# The collector, over what the tests left.
cargo run --quiet --manifest-path "$root/Cargo.toml" -p extent_remote --bin fxdax-gc -- \
    s3://fxdax/test --grace 0
