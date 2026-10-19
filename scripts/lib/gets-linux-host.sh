#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure ranged GETs from object storage (fxdax-gets) on a disposable x86-64
# Linux host, as root, into RESULTS/gets.md (scripts/gets-gce.sh).
#
# Usage: gets-linux-host.sh SOURCE RESULTS URL OBJECT_MIB SECS
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
url=${3:?the URL of a bucket required}
mib=${4:?the size of the object required}
secs=${5:?the seconds each measurement takes required}
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
tree="$work/bench-tree"
mkdir -p "$tree"
# By content, not times, which a tree unpacked from another checkout's has:
# only what differs is written, which makes it newer than what was built from
# it, as cargo and make need, whichever tree the last run built.
rsync -rlpD --checksum --delete --exclude target --exclude /vendor/firecracker/build "$repo/" "$tree/"
{
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    echo "cpus $(nproc)"
    grep MemTotal /proc/meminfo
    rustc --version
} > "$results/host.txt"
CARGO_TARGET_DIR="$work/target" cargo build --release --manifest-path "$tree/Cargo.toml" \
    -p fxdax --bin fxdax-gets
"$work/target/release/fxdax-gets" "$url" "$mib" "$secs" > "$results/gets.md"
