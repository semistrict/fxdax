#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing (scripts/mutants.sh) on a disposable x86-64 Linux host, as
# root, into RESULTS/mutants (scripts/mutants-gce.sh). FXDAX_MUTANTS_SHARD and
# FXDAX_MUTANTS_JOBS are scripts/mutants.sh's; the latter's default here is a
# third of the host's CPUs.
#
# Usage: mutants-linux-host.sh SOURCE RESULTS [CRATE...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
shift 2
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
if [[ "$(cargo nextest --version 2>/dev/null | head -1)" != 'cargo-nextest 0.9.143' ]]; then
    cargo install cargo-nextest --version 0.9.143 --locked
fi
if [[ "$(cargo mutants --version 2>/dev/null)" != 'cargo-mutants 27.1.0' ]]; then
    cargo install cargo-mutants --version 27.1.0 --locked
fi
# cargo-mutants copies the tree, with its target directory, for each job, into TMPDIR: on the
# disk, as /tmp is a tmpfs too small for more than one.
export TMPDIR="$work/tmp"
mkdir -p "$TMPDIR"
tree="$work/mutants-tree"
mkdir -p "$tree"
rsync -a --delete --exclude target --exclude /vendor/firecracker/build "$repo/" "$tree/"
{
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    echo "cpus $(nproc)"
    grep MemTotal /proc/meminfo
    rustc --version
    cargo mutants --version
    echo "shard ${FXDAX_MUTANTS_SHARD:-all}"
} > "$results/host.txt"
export FXDAX_MUTANTS_JOBS=${FXDAX_MUTANTS_JOBS:-$(( $(nproc) / 3 > 0 ? $(nproc) / 3 : 1 ))}
FXDAX_MUTANTS="$results/mutants" bash "$tree/scripts/mutants.sh" "$@"
