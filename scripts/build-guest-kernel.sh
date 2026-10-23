#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build the guest kernel with Fxfs in it (doc-2, TASK-26.1): Amazon Linux's 6.18 kernel, the
# release below, configured as Firecracker's CI configures the 6.18 guests it supports (its
# configurations, in guest/linux/configs, and its docs/kernel-policy.md), with fxdax's series of
# patches (guest/linux/patches) adding fs/fxfs and drivers/virtio_extent from guest/linux, and the
# filesystem's Rust, which cargo builds from guest/rust, linked into fs/fxfs (TASK-39.6).
#
#   build-guest-kernel.sh [arm64|x86_64]
#
# builds for this machine's architecture by default. It runs on Linux, natively; on macOS, run
# it in Lima: scripts/lib/lima.sh build-guest-kernel.sh. The sources and the build live under
# $FXDAX_GUEST_KERNEL_WORK (default: ~/fxdax-guest-kernel), and its out/<arch> gets the kernel
# (Image for arm64, vmlinux for x86-64, as Firecracker loads them) and what builds it elsewhere,
# on the same release: patches/, the series (git am); libfxfs_linux.a, the Rust, for fs/fxfs; and
# fxdax.config, fxdax's options over the configuration. FXDAX_GUEST_KERNEL_PROFILE (below) builds
# one for profiling, FXDAX_GUEST_KERNEL_LOCK_STAT one counting locks' contention.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=${FXDAX_GUEST_KERNEL_WORK:-$HOME/fxdax-guest-kernel}
# The newest of Amazon Linux's 6.18 releases, as Firecracker's resources/rebuild.sh picks them.
kernel_tag=kernel6.18-6.18.54-121.166.amzn2023
kernel_series=6.18
configs="$root/guest/linux/configs"

arch=${1:-$(uname -m)}
case "$arch" in
    arm64 | aarch64)
        arch=arm64 guestarch=aarch64 target=aarch64-unknown-none-softfloat image=arch/arm64/boot/Image
        make_target=Image
        ;;
    x86_64)
        arch=x86_64 guestarch=x86_64 target=x86_64-unknown-none image=vmlinux make_target=vmlinux
        ;;
    *) echo "Unsupported architecture: $arch" >&2; exit 2 ;;
esac
make_args=(ARCH="$arch")
if [[ $guestarch != $(uname -m) ]]; then
    make_args+=(CROSS_COMPILE="$guestarch-linux-gnu-")
fi

# The kernel's sources, once.
src="$work/$kernel_tag-$arch"
if [[ ! -d $src ]]; then
    git clone -q --depth 1 --branch "$kernel_tag" https://github.com/amazonlinux/linux "$src"
fi
out="$work/out/$arch"
mkdir -p "$out"

# fxdax's changes to the kernel, a commit each on its release: the series of guest/linux/patches,
# each with the files of guest/linux it adds. The tree checks out the series' last commit, which
# rewrites only the files that differ from what the tree has, so that a build after the first
# rebuilds only what changed; and the series, as patches, goes to out/<arch>/patches, which a
# kernel built elsewhere applies (git am) on the same release.
kgit() { git -C "$src" "$@"; }
export GIT_AUTHOR_NAME="The fxdax Authors" GIT_AUTHOR_EMAIL="fxdax@echophase.com"
export GIT_COMMITTER_NAME=$GIT_AUTHOR_NAME GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL
# The same series, the same commits.
export GIT_AUTHOR_DATE="2026-10-17T00:00:00Z" GIT_COMMITTER_DATE="2026-10-17T00:00:00Z"
index="$work/series-index-$arch"
series=$(kgit rev-parse "$kernel_tag^{commit}")
GIT_INDEX_FILE=$index kgit read-tree "$series"
# commit PATCH [PATH...]: the next commit of the series, PATCH's change and guest/linux's PATHs
# (files, or directories, without what builds leave there), its message PATCH's header.
commit() {
    local patch="$root/guest/linux/patches/$1" path file blob tree
    shift
    GIT_INDEX_FILE=$index kgit apply --cached "$patch"
    for path in "$@"; do
        (cd "$root/guest/linux" && find "$path" -type f ! -name '*.o' ! -name '.*.cmd' \
            ! -name '*.a' | sort) | while read -r file; do
            blob=$(kgit hash-object -w "$root/guest/linux/$file")
            GIT_INDEX_FILE=$index kgit update-index --add --cacheinfo "100644,$blob,$file"
        done
    done
    tree=$(GIT_INDEX_FILE=$index kgit write-tree)
    # The header, but for its license lines, up to the diff.
    series=$(sed -n '/^diff --git /q; /^# /d; p' "$patch" | kgit commit-tree "$tree" -p "$series")
}
commit 0001-sched-fxfs-tls.patch include/linux/fxfs_tls.h
commit 0002-stack-size.patch
commit 0003-virtio-extent.patch drivers/virtio_extent include/linux/virtio_extent.h
commit 0004-fxfs.patch fs/fxfs
rm -f "$index"
kgit reset -q --hard "$series"
rm -f "$out"/patches/*.patch
kgit format-patch -q --zero-commit --no-signature -o "$out/patches" "$kernel_tag..$series"

# Firecracker's configuration of its 6.18 guests, its guests' and its CI's, as its
# resources/rebuild.sh has them, and fxdax's options, the later overriding the earlier. fxdax.config
# goes to out/<arch> too.
awk 1 "$configs/microvm-kernel-ci-$guestarch-$kernel_series.config" "$configs/ci.config" \
    "$configs/fxdax.config" > "$src/.config"
cp "$configs/fxdax.config" "$out/"
# With FXDAX_GUEST_KERNEL_PROFILE=1, an x86-64 kernel unwinds its stacks by frame pointers, which
# Fxfs's Rust keeps, so that perf's call graphs follow calls through it, not by guesses.
# Frame pointers cost the kernel's own code a little, so a kernel measured has none: build one
# for profiling with its own FXDAX_GUEST_KERNEL_WORK.
if [[ $arch == x86_64 && ${FXDAX_GUEST_KERNEL_PROFILE:-} == 1 ]]; then
    "$src/scripts/config" --file "$src/.config" --disable UNWINDER_GUESS \
        --enable UNWINDER_FRAME_POINTER
fi
# fxdax: with FXDAX_GUEST_KERNEL_DEBUG=1, a kernel that checks its locking (lockdep, sleeping in
# atomic context) and, with KCSAN, its data races, as the followers' torture test runs it
# (TASK-42.7, doc-8 testing 6): slow, so built with its own FXDAX_GUEST_KERNEL_WORK.
if [[ ${FXDAX_GUEST_KERNEL_DEBUG:-} == 1 ]]; then
    "$src/scripts/config" --file "$src/.config" --enable DEBUG_KERNEL --enable PROVE_LOCKING \
        --enable DEBUG_ATOMIC_SLEEP --enable DEBUG_LIST --enable KCSAN
fi
# fxdax: with FXDAX_GUEST_KERNEL_LOCK_STAT=1, a kernel that counts each lock's contention, in
# /proc/lock_stat, which names a hot lock where a sampling profile can't see it, taken with
# interrupts off (fxfs-guest-pgbench-locks, TASK-91.14): slow, so built with its own
# FXDAX_GUEST_KERNEL_WORK.
if [[ ${FXDAX_GUEST_KERNEL_LOCK_STAT:-} == 1 ]]; then
    "$src/scripts/config" --file "$src/.config" --enable DEBUG_KERNEL --enable LOCK_STAT
fi
make -C "$src" "${make_args[@]}" olddefconfig
if [[ ${FXDAX_GUEST_KERNEL_LOCK_STAT:-} == 1 ]]; then
    grep -q '^CONFIG_LOCK_STAT=y' "$src/.config" ||
        { echo "the lock_stat kernel has no CONFIG_LOCK_STAT" >&2; exit 1; }
fi
if [[ ${FXDAX_GUEST_KERNEL_DEBUG:-} == 1 ]]; then
    for option in PROVE_LOCKING DEBUG_ATOMIC_SLEEP KCSAN; do
        grep -q "^CONFIG_$option=y" "$src/.config" ||
            { echo "the debug kernel has no CONFIG_$option" >&2; exit 1; }
    done
fi
grep -q '^CONFIG_FXFS_FS=y' "$src/.config"
grep -q '^CONFIG_VIRTIO_EXTENT=y' "$src/.config"
grep -q '^CONFIG_VIRTIO_PMEM=y' "$src/.config"
grep -q '^CONFIG_SERIAL_8250_CONSOLE=y' "$src/.config"

# The filesystem's Rust, against a sysroot whose std is the kernel's (guest/rust/std).
rustflags=$("$root/scripts/build-guest-sysroot.sh" "$target" "$work/rust-$arch")
(cd "$root/guest/rust" && RUSTFLAGS="$rustflags" CARGO_TARGET_DIR="$work/rust-$arch/target" \
    cargo build --release -p fxfs-linux --target "$target")
cp "$work/rust-$arch/target/$target/release/libfxfs_linux.a" "$src/fs/fxfs/"
# The kernel's memcpy and friends for Rust too, not compiler_builtins' generic ones: those are in
# members of their own, which would otherwise define them for the whole kernel (arm64's own are
# weak), copying a byte at a time where the kernel's copy words.
string_functions='memcpy|memmove|memset|memcmp|bcmp|strlen'
archive="$src/fs/fxfs/libfxfs_linux.a"
nm -A --defined-only "$archive" 2>/dev/null |
    awk -v names="^($string_functions)\$" '$NF ~ names { split($1, f, ":"); print f[2] }' |
    sort -u | while read -r member; do ar d "$archive" "$member"; done
cp "$archive" "$out/"

make -C "$src" "${make_args[@]}" -j"$(nproc)" KBUILD_BUILD_USER=root KBUILD_BUILD_HOST=fxdax \
    "$make_target"
# The kernel's own memcpy, which Rust calls, is in the kernel: not Rust's. nm's listing whole,
# before grep reads it: errexit stops at nm failing, and nothing stops nm part way.
rust_symbols=$(nm "$src/fs/fxfs/fxfs_rust.o")
if grep -E " [TtWw] ($string_functions)\$" <<< "$rust_symbols" >&2; then
    echo "fxfs_rust.o defines the string functions above, which must be the kernel's" >&2
    exit 1
fi
cp "$src/$image" "$out/"
echo "$out/$(basename "$image")"
