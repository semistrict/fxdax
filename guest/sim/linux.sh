#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# Runs a command in this directory on Linux, as its scripts need: here, on Linux, with KERNEL and
# BUILD as the Makefile takes them; elsewhere in a container of Dockerfile's image, with the
# repository mounted where it is here, and KERNEL and BUILD in the volume fxdax-sim, the kernel's
# tree fetched there once (kernel-tree.sh). The container runs without seccomp's filter: a test
# runs itself again with ASLR off, by personality(2), which Docker's default filter refuses.
#
#   linux.sh COMMAND [ARGUMENT...]        such as linux.sh ./mutants.sh, linux.sh ./zloop.sh -s 300

set -euo pipefail
cd "$(dirname "$0")"
(($#)) || { sed -n '4,11p' "$0" >&2; exit 2; }

if [[ $(uname -s) == Linux ]]; then
    exec "$@"
fi
docker build -q -t fxdax-sim - < Dockerfile > /dev/null
repo=$(cd ../.. && pwd)
exec docker run --rm --init --security-opt seccomp=unconfined -v "$repo:$repo" -v fxdax-sim:/sim \
    -w "$PWD" -e KERNEL=/sim/kernel -e BUILD=/sim/build fxdax-sim \
    bash -c './kernel-tree.sh "$KERNEL" && exec "$@"' linux.sh "$@"
