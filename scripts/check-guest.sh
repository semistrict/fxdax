#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The quick check of a batch of the guest's changes, between builds: type-checks the guest
# filesystem's Rust (cargo check, no code), and compiles guest/linux's C, fs/fxfs and
# drivers/virtio_extent, object by object, in the kernel tree build-guest-kernel.sh configured
# before, building and linking no kernel. A kernel built from the tree after puts its own series
# back (build-guest-kernel.sh resets it). It runs on Linux; on macOS, run it in Lima:
# scripts/lib/lima.sh check-guest.sh. Arguments: the architecture (default: this machine's), as
# build-guest-kernel.sh's.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=${FXDAX_GUEST_KERNEL_WORK:-$HOME/fxdax-guest-kernel}
kernel_tag=kernel6.18-6.18.54-121.166.amzn2023

arch=${1:-$(uname -m)}
case "$arch" in
    arm64 | aarch64) arch=arm64 guestarch=aarch64 target=aarch64-unknown-none-softfloat ;;
    x86_64) arch=x86_64 guestarch=x86_64 target=x86_64-unknown-none ;;
    *) echo "Unsupported architecture: $arch" >&2; exit 2 ;;
esac
make_args=(ARCH="$arch")
if [[ $guestarch != $(uname -m) ]]; then
    make_args+=(CROSS_COMPILE="$guestarch-linux-gnu-")
fi
src="$work/$kernel_tag-$arch"
if [[ ! -f $src/.config ]]; then
    echo "no configured kernel tree at $src: build one first (build-guest-kernel.sh $arch)" >&2
    exit 1
fi

# The Rust, against the sysroot a build uses, its target directory too: no code.
rustflags=$("$root/scripts/build-guest-sysroot.sh" "$target" "$work/rust-$arch")
(cd "$root/guest/rust" && RUSTFLAGS="$rustflags" CARGO_TARGET_DIR="$work/rust-$arch/target" \
    cargo check --release -p fxfs-linux --target "$target")

# guest/linux's files over the tree's, as the series adds them (build-guest-kernel.sh), and their
# objects compiled.
objects=()
for path in include/linux/fxfs_tls.h include/linux/virtio_extent.h drivers/virtio_extent \
    fs/fxfs; do
    while read -r file; do
        mkdir -p "$src/$(dirname "$file")"
        cp "$root/guest/linux/$file" "$src/$file"
        if [[ $file == *.c ]]; then
            objects+=("${file%.c}.o")
        fi
    done < <(cd "$root/guest/linux" && find "$path" -type f \( -name '*.c' -o -name '*.h' \
        -o -name Makefile -o -name Kconfig \) | sort)
done
make -C "$src" "${make_args[@]}" -j"$(nproc)" "${objects[@]}"
echo "guest check: ok ($arch)"
