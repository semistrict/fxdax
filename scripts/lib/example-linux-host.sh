#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs one of the workspace's examples, an optimized build, on a disposable x86-64
# Linux host, as root, into RESULTS/output.txt (scripts/example-gce.sh). With
# FXDAX_PERF=record, perf samples it, and RESULTS/perf.txt says where its
# threads' time went.
#
# Usage: example-linux-host.sh SOURCE RESULTS EXAMPLE [ARG...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
example=${3:?an example required}
shift 3
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
tree="$work/example-tree"
mkdir -p "$tree"
rsync -a --delete --exclude target --exclude /vendor/firecracker/build "$repo/" "$tree/"
{
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    echo "cpus $(nproc)"
    grep MemTotal /proc/meminfo
    rustc --version
} > "$results/host.txt"
# Under perf, frame pointers, which its call graphs walk, and symbols; a target of its own, as
# the flags differ.
target="$work/target"
if [[ ${FXDAX_PERF:-} == record ]]; then
    target="$work/target-perf"
    export RUSTFLAGS="-C force-frame-pointers=yes" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
fi
CARGO_TARGET_DIR="$target" \
    cargo build --release --manifest-path "$tree/Cargo.toml" --example "$example"
binary="$target/release/examples/$example"
if [[ ${FXDAX_PERF:-} != record ]]; then
    # FXDAX_EXAMPLE_RUNS times, one after another, to see how much runs differ.
    for _ in $(seq "${FXDAX_EXAMPLE_RUNS:-1}"); do
        "$binary" "$@"
    done > "$results/output.txt"
    exit 0
fi
if ! command -v perf > /dev/null; then
    apt-get -o DPkg::Lock::Timeout=600 install -y -qq linux-tools-common \
        "linux-tools-$(uname -r)" > /dev/null
fi
data="$work/perf.data"
perf record -o "$data" -F 4999 --call-graph fp -- "$binary" "$@" > "$results/output.txt"
source "$(dirname "${BASH_SOURCE[0]}")/perf-report.sh"
perf_report "$data" "$results/perf"
rm -f "$data"
