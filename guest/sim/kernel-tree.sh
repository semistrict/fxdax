#!/bin/bash
# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# The kernel's sources this directory builds against, in TREE, where there is no configured tree of
# the guest's kernel: the release scripts/build-guest-kernel.sh builds, those of its files the
# Makefile's REAL_SOURCES and REAL_HEADERS name and what they include, with the series's patches to
# fs/dax.c. Needs nothing built, and no configuration.
#
#   kernel-tree.sh TREE

set -euo pipefail
cd "$(dirname "$0")"

tree=${1:?usage: kernel-tree.sh TREE}
tag=$(sed -n 's/^kernel_tag=//p' ../../scripts/build-guest-kernel.sh)
if [[ -f $tree/.sim-tree-$tag ]]; then
    exit 0
fi
git clone -q --depth 1 --filter=blob:none --sparse --branch "$tag" \
    https://github.com/amazonlinux/linux "$tree"
git -C "$tree" sparse-checkout set --no-cone /fs/dax.c /fs/iomap/ /include/ /lib/xarray.c \
    /lib/radix-tree.h /lib/genalloc.c
for patch in ../linux/patches/000[5-9]-*.patch ../linux/patches/001[01]-*.patch; do
    git -C "$tree" apply "$(realpath "$patch")"
done
touch "$tree/.sim-tree-$tag"
