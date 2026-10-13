#!/usr/bin/env bash
# Build the guest kernel with Fxfs in it (doc-2, TASK-26.1): libkrunfw's kernel, as the libkrunfw
# release below patches and configures it, with fs/fxfs from guest/linux built in, and the
# filesystem's Rust, which cargo builds from guest/rust, linked into fs/fxfs.
#
#   build-guest-kernel.sh [arm64|x86_64]
#
# builds for this machine's architecture by default. It runs on Linux, natively; on macOS, run
# it in Lima: scripts/lib/lima.sh build-guest-kernel.sh. The sources and the build live under
# $FXDAX_GUEST_KERNEL_WORK (default: ~/fxdax-guest-kernel), and the kernel ends up in its
# out/<arch>: Image for arm64, vmlinux for x86-64, as libkrun loads them.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=${FXDAX_GUEST_KERNEL_WORK:-$HOME/fxdax-guest-kernel}
libkrunfw_version=v5.6.2

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

# libkrunfw's patched and configured kernel sources, once.
libkrunfw="$work/libkrunfw-$libkrunfw_version"
if [[ ! -d $libkrunfw ]]; then
    git clone -q --depth 1 --branch "$libkrunfw_version" https://github.com/libkrun/libkrunfw \
        "$libkrunfw"
fi
kernel=$(sed -n 's/^KERNEL_VERSION = //p' "$libkrunfw/Makefile")
src="$work/$kernel-$arch"
if [[ ! -d $src ]]; then
    make -C "$libkrunfw" "${make_args[@]}" GUESTARCH="$guestarch" "$kernel"
    mv "$libkrunfw/$kernel" "$src"
fi

# fs/fxfs, and the lines that build it.
rsync -a --delete --exclude libfxfs_linux.a --exclude '*.o' --exclude '.*.cmd' \
    "$root/guest/linux/fs/fxfs/" "$src/fs/fxfs/"
grep -q '^source "fs/fxfs/Kconfig"' "$src/fs/Kconfig" ||
    sed -i 's|^source "fs/fuse/Kconfig"|source "fs/fxfs/Kconfig"\n&|' "$src/fs/Kconfig"
grep -q 'CONFIG_FXFS_FS' "$src/fs/Makefile" ||
    echo 'obj-$(CONFIG_FXFS_FS)		+= fxfs/' >> "$src/fs/Makefile"
"$src/scripts/config" --file "$src/.config" --enable FXFS_FS
make -C "$src" "${make_args[@]}" olddefconfig
grep -q '^CONFIG_FXFS_FS=y' "$src/.config"

# The filesystem's Rust.
(cd "$root/guest/rust" && cargo build --release -p fxfs-linux --target "$target")
cp "$root/guest/rust/target/$target/release/libfxfs_linux.a" "$src/fs/fxfs/"

make -C "$src" "${make_args[@]}" -j"$(nproc)" KBUILD_BUILD_USER=root KBUILD_BUILD_HOST=fxdax \
    "$make_target"
out="$work/out/$arch"
mkdir -p "$out"
cp "$src/$image" "$out/"
echo "$out/$(basename "$image")"
