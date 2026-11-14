#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Measure fxdax-gc's sweeps over a bucket of files of many cells (scripts/gc-sweep.sh) on an x86-64
# Linux host, as root, into RESULTS (scripts/gc-sweep-host.sh), its buckets in the work directory:
# this tree's fxdax-gc, or, given SIDE=TREEs, each TREE's, by this tree's scripts (TASK-395.17).
#
# Usage: gc-sweep-linux-host.sh SOURCE RESULTS [SIDE=TREE...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
shift 2
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
host_record > "$results/host.txt"
"$repo/scripts/gc-sweep.sh" "$work/gc-sweep" "$results" "$@"
