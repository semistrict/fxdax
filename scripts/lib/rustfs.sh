# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced by the scripts that keep what they test in an S3 bucket on this machine: RustFS
# (github.com/rustfs/rustfs, Apache-2.0), an S3 server, in a container (docker, or podman:
# scripts/lib/containers.sh). scripts/test-rustfs.sh runs the branches' tests against it,
# scripts/test-e2e-rustfs.sh the guest tests, their extent daemons' branches in it.
#
# FXDAX_RUSTFS_IMAGE names its image (by default rustfs/rustfs, at the version tested), which has
# curl, its health check's client.

source "$(dirname "${BASH_SOURCE[0]}")/containers.sh"

# rustfs_start [DATA]: starts RustFS on a free port of 127.0.0.1, ready once its readiness
# endpoint, asked inside the container, says so, and makes its bucket `fxdax`. Its objects are in
# DATA, a directory of this machine's, if given, as this user's, the container run as this user;
# else in the container's own, gone with it. It exports what S3's clients (object_store, the AWS
# CLI) find a server and its credentials by, AWS_ENDPOINT and AWS_ACCESS_KEY_ID among them, and
# sets rustfs_engine and rustfs_server, which rustfs_stop stops. It fails, the container stopped,
# if the server doesn't start or the bucket can't be made.
rustfs_start() {
    local data=${1:-} image="${FXDAX_RUSTFS_IMAGE:-rustfs/rustfs:1.0.4}" port volume=()
    port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
    # Test credentials, for this server alone.
    export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
    export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
    export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true
    if [[ -n $data ]]; then
        volume=(--user "$(id -u):$(id -g)" -v "$data:/data" -e RUSTFS_VOLUMES=/data)
    fi
    rustfs_engine="$(container_engine)" || return
    # Ready as its readiness endpoint answers 200 (TASK-239: a probe from here, of /, retried all
    # errors into /dev/null, and curl's truncation of it failed, exiting 23).
    local health='curl -fsS -o /dev/null http://127.0.0.1:9000/health/ready'
    rustfs_server="$(healthy_container "$rustfs_engine" "$health" \
        -p "127.0.0.1:$port:9000" ${volume[@]+"${volume[@]}"} \
        -e RUSTFS_ACCESS_KEY="$AWS_ACCESS_KEY_ID" -e RUSTFS_SECRET_KEY="$AWS_SECRET_ACCESS_KEY" \
        "$image")" || return
    # The bucket, made with a request curl signs as S3's API wants (SigV4).
    if ! curl -fsS -X PUT --aws-sigv4 "aws:amz:$AWS_REGION:s3" \
        --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$AWS_ENDPOINT/fxdax"; then
        rustfs_stop
        return 1
    fi
}

# rustfs_stop: stops the server rustfs_start started, if it did.
rustfs_stop() {
    [[ -n ${rustfs_server:-} ]] || return 0
    "$rustfs_engine" stop "$rustfs_server" > /dev/null
    rustfs_server=
}
