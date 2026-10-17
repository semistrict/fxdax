#!/usr/bin/env bash
# Run the branches' tests against RustFS, an S3 server (github.com/rustfs/rustfs, Apache-2.0), on
# this machine: start it on a directory of its own, create a bucket with the AWS CLI, run the S3
# tests (ignored in the workspace's run), and stop it. Arguments are passed to scripts/test.sh.
#
# FXDAX_RUSTFS names its binary, built from its sources with
# `cargo build --release --bin rustfs` (by default, ~/src/rustfs's).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
rustfs="${FXDAX_RUSTFS:-$HOME/src/rustfs/target/release/rustfs}"
if [ ! -x "$rustfs" ]; then
    echo "no RustFS binary at $rustfs: build it, or set FXDAX_RUSTFS" >&2
    exit 1
fi

# The server's data and log, under target/, which a later run replaces.
data="$root/target/rustfs"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
mkdir -p "$data/volume-$port"

# Test credentials, for this server alone.
export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true

"$rustfs" server --address "127.0.0.1:$port" \
    --access-key "$AWS_ACCESS_KEY_ID" --secret-key "$AWS_SECRET_ACCESS_KEY" \
    "$data/volume-$port" >"$data/server-$port.log" 2>&1 &
server=$!
trap 'kill "$server"; wait "$server" 2>/dev/null || true' EXIT

# Up once it answers anything.
curl -s --retry 60 --retry-connrefused --retry-delay 1 -o /dev/null "$AWS_ENDPOINT/"
aws --endpoint-url "$AWS_ENDPOINT" s3 mb s3://fxdax >/dev/null

FXDAX_S3_URL=s3://fxdax/test "$root/scripts/test.sh" -p extent_remote \
    --run-ignored only -E 'test(/rustfs/)' "$@"
