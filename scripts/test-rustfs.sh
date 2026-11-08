#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run the branches' tests against RustFS, an S3 server (github.com/rustfs/rustfs, Apache-2.0), on
# this machine: start it in a container (docker, or podman: scripts/lib/containers.sh), ready once
# its health check, its readiness endpoint asked inside the container, says so; create a bucket,
# run the S3 tests (ignored in the workspace's run), collect what they left, and stop it. CI's
# object-stores job runs it (TASK-149.20). Arguments are passed to cargo nextest.
#
# FXDAX_RUSTFS_IMAGE names its image (by default rustfs/rustfs, at the version tested), which has
# curl, its health check's client.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="${FXDAX_RUSTFS_IMAGE:-rustfs/rustfs:1.0.4}"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
# Test credentials, for this server alone.
export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true
source "$root/scripts/lib/containers.sh"
engine="$(container_engine)"
# Ready as its readiness endpoint answers 200 (TASK-239: a probe from here, of /, retried all
# errors into /dev/null, and curl's truncation of it failed, exiting 23).
server="$(healthy_container "$engine" 'curl -fsS -o /dev/null http://127.0.0.1:9000/health/ready' \
    -p "127.0.0.1:$port:9000" \
    -e RUSTFS_ACCESS_KEY="$AWS_ACCESS_KEY_ID" -e RUSTFS_SECRET_KEY="$AWS_SECRET_ACCESS_KEY" \
    "$image")"
trap '"$engine" stop "$server" >/dev/null' EXIT
# The bucket, made with a request curl signs as S3's API wants (SigV4).
curl -fsS -X PUT --aws-sigv4 "aws:amz:$AWS_REGION:s3" \
    --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$AWS_ENDPOINT/fxdax"
FXDAX_S3_URL=s3://fxdax/test cargo nextest run --manifest-path "$root/Cargo.toml" \
    -p extent_remote -p fxdax --run-ignored only -E 'test(/rustfs/)' "$@"
# The collector, over what the tests left.
cargo run --quiet --manifest-path "$root/Cargo.toml" -p extent_remote --bin fxdax-gc -- \
    s3://fxdax/test --grace 0
