#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Mutation testing (scripts/mutants.sh) on a disposable x86-64 Linux host, as
# root, into RESULTS/mutants (scripts/mutants-gce.sh). FXDAX_MUTANTS_SHARD and
# FXDAX_MUTANTS_JOBS are scripts/mutants.sh's, as is FXDAX_MUTANTS_LIST, a path in SOURCE; the latter's default here is a
# third of the host's CPUs.
#
# Usage: mutants-linux-host.sh SOURCE RESULTS [CRATE...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
shift 2
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
host_tree mutants-tree
host_need cargo-nextest cargo-mutants
# cargo-mutants copies the tree, with its target directory, for each job, into TMPDIR: on the
# disk, as /tmp is a tmpfs too small for more than one.
export TMPDIR="$work/tmp"
mkdir -p "$TMPDIR"
{
    host_record
    echo "shard ${FXDAX_MUTANTS_SHARD:-all}"
} > "$results/host.txt"
export FXDAX_MUTANTS_JOBS=${FXDAX_MUTANTS_JOBS:-$(( $(nproc) / 3 > 0 ? $(nproc) / 3 : 1 ))}
if [[ -n ${FXDAX_MUTANTS_LIST:-} ]]; then
    export FXDAX_MUTANTS_LIST="$repo/$FXDAX_MUTANTS_LIST"
fi
FXDAX_MUTANTS="$results/mutants" bash "$repo/scripts/mutants.sh" "$@"
