#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs one of fxdax's examples, an optimized build, on a disposable x86-64 Linux
# host, as root, into RESULTS/output.txt (scripts/example-gce.sh). With
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
    cargo build --release --manifest-path "$tree/Cargo.toml" -p fxdax --example "$example"
binary="$target/release/examples/$example"
if [[ ${FXDAX_PERF:-} != record ]]; then
    "$binary" "$@" > "$results/output.txt"
    exit 0
fi
if ! command -v perf > /dev/null; then
    apt-get -o DPkg::Lock::Timeout=600 install -y -qq linux-tools-common \
        "linux-tools-$(uname -r)" > /dev/null
fi
data="$work/perf.data"
perf record -o "$data" -F 4999 --call-graph fp -- "$binary" "$@" > "$results/output.txt"
{
    echo "# By thread"
    perf report -i "$data" --stdio --no-children -g none --sort comm --percent-limit 0.05
    echo "# By thread and symbol"
    perf report -i "$data" --stdio --no-children -g none --sort comm,dso,sym --percent-limit 0.2
    echo "# Call graphs of the symbols over 1%, callers first"
    perf report -i "$data" --stdio --children -g caller,0.5,callee --sort sym --percent-limit 1
} > "$results/perf.txt" 2> "$results/perf-report.log"
# Each busy thread's time, by what it called, callers first: what each request costs it.
perf report -i "$data" --stdio --no-children -g none --sort comm --percent-limit 2 2>/dev/null |
    sed -n 's/^ *[0-9.]*%  *\(.*[^ ]\) *$/\1/p' | while read -r comm; do
    name=$(echo "$comm" | tr -c 'A-Za-z0-9\n' -)
    perf report -i "$data" --stdio --comm "$comm" --children -g caller,2,callee,function,percent \
        --sort sym --percent-limit 3 > "$results/perf-$name.txt" 2>> "$results/perf-report.log"
done
rm -f "$data"
