#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run scripts/test.sh on Linux inside a Lima VM. Lima mounts the host home
# read-only, so the repo is copied into the VM first.
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
args=
if (( $# > 0 )); then args=$(printf ' %q' "$@"); fi

limactl shell "$instance" -- bash -lc "
    set -euo pipefail
    rsync -a --delete --exclude target --exclude .git '$repo/' ~/fxdax-test/
    ~/fxdax-test/scripts/test.sh$args
"
