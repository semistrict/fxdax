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
host_setup
host_tree example-tree
# Under perf (FXDAX_PERF=record): perf itself, and symbols, in a target of their own, as the
# profile differs. Its call graphs walk frame pointers, which every build here has
# (.cargo/config.toml).
target="$repo/target"
if [[ ${FXDAX_PERF:-} == record ]]; then
    host_need perf
    target="$repo/target/perf"
    export CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
fi
host_record > "$results/host.txt"
CARGO_TARGET_DIR="$target" \
    cargo build --release --manifest-path "$repo/Cargo.toml" --example "$example"
binary="$target/release/examples/$example"
if [[ ${FXDAX_PERF:-} != record ]]; then
    # FXDAX_EXAMPLE_RUNS times, one after another, to see how much runs differ.
    for _ in $(seq "${FXDAX_EXAMPLE_RUNS:-1}"); do
        "$binary" "$@"
    done > "$results/output.txt"
    exit 0
fi
data="$work/perf.data"
perf record -o "$data" -F 4999 --call-graph fp -- "$binary" "$@" > "$results/output.txt"
source "$(dirname "${BASH_SOURCE[0]}")/perf-report.sh"
perf_report "$data" "$results/perf"
rm -f "$data"
