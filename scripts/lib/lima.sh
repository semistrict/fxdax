#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run one of the repo's scripts on Linux inside a Lima VM: lima.sh SCRIPT [ARG...]
# runs scripts/SCRIPT with the arguments there. Lima mounts the host home
# read-only, so the repo is copied into the VM first, to ~/fxdax-test, by
# content, as a host's tree is (host_tree, scripts/lib/linux-host-setup.sh);
# what the script leaves behind stays in the VM.
#
# LIBCLANG_PATH, which building Firecracker's bindings needs where llvm-config doesn't find
# libclang, passes through if set.
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
script=$(printf '%q' "${1:?script required}")
shift
args=
if (( $# > 0 )); then args=$(printf ' %q' "$@"); fi
library=$(printf '%q' "$repo/scripts/lib/linux-host-setup.sh")
source=$(printf '%q' "$repo")
libclang=$(printf '%q' "${LIBCLANG_PATH:-}")

limactl shell "$instance" -- bash -lc "
    set -euo pipefail
    source $library
    work=~ repo=$source
    host_tree fxdax-test
    if [[ -n $libclang ]]; then export LIBCLANG_PATH=$libclang; fi
    ~/fxdax-test/scripts/$script$args
"
