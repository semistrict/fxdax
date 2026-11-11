# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced by the scripts that keep what they test in an S3 bucket on this machine: RustFS
# (github.com/rustfs/rustfs, Apache-2.0), an S3 server, run as a process of this script's, its
# release's static binary, fetched once and checked against its digest. scripts/test-rustfs.sh
# runs the branches' tests against it, scripts/test-e2e-rustfs.sh the guest tests, their extent
# daemons' branches in it, and scripts/sandboxes.sh LangSmith's sandboxes' run. A process, not a
# container: one way to start it on every host, CI's runners, ubu1 and macOS; and on ubu1 its CPU,
# memory and I/O are the run's own, in its fxjob cgroup and on its CPUs, as LangSmith's RustFS
# shared their runner with their sandboxes (TASK-330.5), where a container's ran in the engine's
# cgroup, beside other agents' jobs.
#
# FXDAX_RUSTFS_BIN names a RustFS binary to run instead of the release's.

# The release run, and each platform's archive's SHA-256 (its SHA256SUMS).
RUSTFS_VERSION=1.0.4

# rustfs_binary prints the path of the release's binary for this platform, fetching it once into
# the cache (XDG_CACHE_HOME's, or ~/.cache's, fxdax/rustfs-VERSION).
rustfs_binary() {
    if [[ -n ${FXDAX_RUSTFS_BIN:-} ]]; then
        echo "$FXDAX_RUSTFS_BIN"
        return
    fi
    local platform sha256 dir
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64)
            platform=linux-x86_64-musl
            sha256=4924ad4b699b9f59943051efedae2d1eb6d4cac503a1b0384c4b25f7c2c31b5c ;;
        Linux-aarch64)
            platform=linux-aarch64-musl
            sha256=81414fd0e41969f4f5c2e0555951cee76d3f224c112c269c80f52b31276f187a ;;
        Darwin-arm64)
            platform=macos-aarch64
            sha256=49057482769d7904ef95dc40457d71122f48cbb8e9e0725a0817a3ec6daf8270 ;;
        *) echo "RustFS $RUSTFS_VERSION has no release for $(uname -s) $(uname -m)" >&2; return 1 ;;
    esac
    dir="${XDG_CACHE_HOME:-$HOME/.cache}/fxdax/rustfs-$RUSTFS_VERSION-$platform"
    if [[ ! -x $dir/rustfs ]]; then
        mkdir -p "$dir" || return
        curl -fsSL --retry 3 -o "$dir.zip" "https://github.com/rustfs/rustfs/releases/download/$RUSTFS_VERSION/rustfs-$platform-v$RUSTFS_VERSION.zip" ||
            return
        echo "$sha256  $dir.zip" | shasum -a 256 --check --quiet || { rm -f "$dir.zip"; return 1; }
        python3 -m zipfile -e "$dir.zip" "$dir" && chmod +x "$dir/rustfs" || return
        rm "$dir.zip"
    fi
    echo "$dir/rustfs"
}

# rustfs_start [DATA]: starts RustFS on a free port of 127.0.0.1, ready once its readiness
# endpoint says so, and makes its bucket `fxdax`. Its objects are in DATA, a directory, if given;
# else in one of its own, gone as it stops. Its log is FXDAX_RUSTFS_LOG's file if set, else one of
# its own, said if it fails to start. It exports what S3's clients (object_store, the AWS CLI) find
# a server and its credentials by, AWS_ENDPOINT and AWS_ACCESS_KEY_ID among them, and sets
# rustfs_server, its pid, which rustfs_stop stops. It fails, the server stopped, if the server
# doesn't get ready within RUSTFS_READY_WITHIN seconds (120) or the bucket can't be made.
rustfs_start() {
    local data=${1:-} port bin ready=0 waited
    bin=$(rustfs_binary) || return
    port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
    # Test credentials, for this server alone.
    export AWS_ACCESS_KEY_ID=fxdaxtest AWS_SECRET_ACCESS_KEY=fxdaxtestsecret
    export AWS_REGION=us-east-1 AWS_DEFAULT_REGION=us-east-1
    export AWS_ENDPOINT="http://127.0.0.1:$port" AWS_ALLOW_HTTP=true
    rustfs_own_data=
    if [[ -z $data ]]; then
        data=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-rustfs.XXXXXX") || return
        rustfs_own_data=$data
    fi
    rustfs_log=${FXDAX_RUSTFS_LOG:-}
    rustfs_own_log=
    if [[ -z $rustfs_log ]]; then
        rustfs_log=$(mktemp "${TMPDIR:-/tmp}/fxdax-rustfs-log.XXXXXX") || return
        rustfs_own_log=$rustfs_log
    fi
    "$bin" server --address "127.0.0.1:$port" --access-key "$AWS_ACCESS_KEY_ID" \
        --secret-key "$AWS_SECRET_ACCESS_KEY" "$data" > "$rustfs_log" 2>&1 < /dev/null &
    rustfs_server=$!
    # Its readiness endpoint, asked directly: no engine's port forwarding between, which took
    # connections before the server did and cut them short (TASK-239).
    for (( waited = 0; waited < ${RUSTFS_READY_WITHIN:-120} * 4; waited++ )); do
        if ! kill -0 "$rustfs_server" 2> /dev/null; then break; fi
        if curl -fsS -o /dev/null --max-time 2 "$AWS_ENDPOINT/health/ready" 2> /dev/null; then
            ready=1
            break
        fi
        sleep 0.25
    done
    if (( ! ready )); then
        echo "RustFS didn't get ready within ${RUSTFS_READY_WITHIN:-120} s:" >&2
        tail -n 40 "$rustfs_log" >&2
        rustfs_stop
        return 1
    fi
    # The bucket, made with a request curl signs as S3's API wants (SigV4).
    if ! curl -fsS -X PUT --aws-sigv4 "aws:amz:$AWS_REGION:s3" \
        --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" "$AWS_ENDPOINT/fxdax"; then
        rustfs_stop
        return 1
    fi
}

# rustfs_stop: stops the server rustfs_start started, if it did, and removes what was its own.
rustfs_stop() {
    [[ -n ${rustfs_server:-} ]] || return 0
    kill "$rustfs_server" 2> /dev/null || true
    wait "$rustfs_server" 2> /dev/null || true
    rustfs_server=
    [[ -z ${rustfs_own_data:-} ]] || rm -r "$rustfs_own_data"
    [[ -z ${rustfs_own_log:-} ]] || rm -f "$rustfs_own_log"
}
