# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# What linux.sh runs this directory in where the host isn't Linux: clang and lld to build it, git
# for kernel-tree.sh, and what its scripts use.
FROM debian:trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates clang git libclang-rt-dev lld make patch \
    && rm -rf /var/lib/apt/lists/*
