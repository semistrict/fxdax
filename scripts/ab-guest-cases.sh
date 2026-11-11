#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# An interleaved A/B of guest cases on this Linux host (ubu1), two sides' prebuilt artifacts
# (scripts/test-e2e.sh's FXDAX_E2E_BUILD_ONLY) booting one guest kernel: ROUNDS rounds, each
# running every case on both sides, A then B in odd rounds and B then A in even ones, so that what
# drifts on the shared host over the hours moves both sides alike (TASK-310). Each run is a
# measurement of its own under `fxjob --bench` (cores 4-7 its own, FXDAX_AB_MEM of memory, 20G by
# default), its results in OUT/<case>/<side>-<round>; scripts/ab-guest-summary.py compares them.
# What a run leaves of its stores, bucket, part cache and VMs' root filesystems goes once it ends
# (a shared-disk run leaves some 25 GB), but with FXDAX_AB_KEEP_DATA=1: its logs and stats stay.
#
#   scripts/ab-guest-cases.sh OUT A_NAME=A_ARTIFACTS B_NAME=B_ARTIFACTS ROUNDS CASE...
#
# A case may name the environment its runs take, CASE@VAR=VALUE[,VAR=VALUE...] (its results then
# under CASE@VAR=VALUE...), as fxfs-guest-shared-disk@FXDAX_SHARED_DISK_FSYNC=1; OUT short enough
# for the daemon's control socket in each run's directory (checked first). FXDAX_GUEST_KERNEL
# names the guest kernel. A run that fails is kept, said, and the rounds go on.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
(( $# >= 5 )) || { sed -n '6,17p' "$0" >&2; exit 2; }
out=$1 a=$2 b=$3 rounds=$4
shift 4
[[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "ROUNDS is a count: $rounds" >&2; exit 2; }
: "${FXDAX_GUEST_KERNEL:?FXDAX_GUEST_KERNEL names the guest kernel}"
for side in "$a" "$b"; do
    [[ $side =~ ^[a-z0-9][a-z0-9-]*=.+$ ]] || { echo "a side is NAME=ARTIFACTS: $side" >&2; exit 2; }
    [[ -f ${side#*=}/artifacts.txt ]] || { echo "${side#*=} holds no artifacts" >&2; exit 2; }
done
[[ ${a%%=*} != "${b%%=*}" ]] || { echo "the sides need names of their own" >&2; exit 2; }
mkdir -p "$out"
out=$(cd "$out" && pwd)
# The daemon's control socket is made in each run's directory: its path must fit a Unix socket's.
for spec in "$@"; do
    for side in "$a" "$b"; do
        socket="$out/$spec/${side%%=*}-$rounds/extentd.control"
        (( ${#socket} < 108 )) || { echo "$socket: too long for a Unix socket" >&2; exit 2; }
    done
done

# Runs case spec $1 on side $2 (NAME=ARTIFACTS), round $3.
run() {
    local spec=$1 name=${2%%=*} artifacts=${2#*=} round=$3 case=${1%%@*} env=()
    [[ $spec == *@* ]] && IFS=, read -r -a env <<< "${spec#*@}"
    local results=$out/$spec/$name-$round
    mkdir -p "$results"
    echo "ab: round $round, $spec, $name" >&2
    if ! env "${env[@]}" FXDAX_E2E_PREBUILT="$artifacts" FXDAX_E2E_RESULTS="$results" \
        fxjob --bench --name "ab-$name-$round" --mem "${FXDAX_AB_MEM:-20G}" -- \
        "$repo/scripts/test-e2e.sh" "$case" > "$results/run.log" 2>&1; then
        echo "ab: round $round, $spec, $name failed: $results/run.log" >&2
    fi
    if [[ -z ${FXDAX_AB_KEEP_DATA:-} ]]; then
        find "$results" -type d \( -name 'store-*' -o -name extentd.bucket -o -name extentd-cache \
            -o -name rootfs \) -prune -exec rm -r {} +
        find "$results" -name '*.rootfs.ext4' -delete
    fi
}

for (( round = 1; round <= rounds; round++ )); do
    for spec in "$@"; do
        if (( round % 2 )); then
            run "$spec" "$a" "$round"
            run "$spec" "$b" "$round"
        else
            run "$spec" "$b" "$round"
            run "$spec" "$a" "$round"
        fi
    done
done
echo "ab: done: $out" >&2
