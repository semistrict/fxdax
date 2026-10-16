#!/usr/bin/env bash
# Build the guest kernel with Fxfs in it (doc-2, TASK-26.1): libkrunfw's kernel, as the libkrunfw
# release below patches and configures it, with fs/fxfs and drivers/virtio_extent from guest/linux
# built in, and the filesystem's Rust, which cargo builds from guest/rust, linked into fs/fxfs.
#
#   build-guest-kernel.sh [arm64|x86_64]
#
# builds for this machine's architecture by default. It runs on Linux, natively; on macOS, run
# it in Lima: scripts/lib/lima.sh build-guest-kernel.sh. The sources and the build live under
# $FXDAX_GUEST_KERNEL_WORK (default: ~/fxdax-guest-kernel), and the kernel ends up in its
# out/<arch>: Image for arm64, vmlinux for x86-64, as libkrun loads them. FXDAX_GUEST_KERNEL_PROFILE
# (below) builds one for profiling.
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

# fxdax's patches to the kernel: a slot in each task for fs/fxfs's Rust's thread-local values. A
# tree patched before keeps those patches.
cp "$root/guest/linux/include/linux/fxfs_tls.h" "$src/include/linux/"
grep -q fxfs_tls "$src/include/linux/sched.h" ||
    patch -d "$src" -p1 --no-backup-if-mismatch < "$root/guest/linux/task-fxfs-tls.patch"
# And tasks' stacks of 64 KiB, which Fxfs's operations need (TASK-26.9).
grep -q fxdax "$src/arch/arm64/include/asm/memory.h" ||
    patch -d "$src" -p1 --no-backup-if-mismatch < "$root/guest/linux/task-stack-size.patch"

# fs/fxfs, and the lines that build it.
rsync -a --delete --exclude libfxfs_linux.a --exclude '*.o' --exclude '.*.cmd' \
    "$root/guest/linux/fs/fxfs/" "$src/fs/fxfs/"
grep -q '^source "fs/fxfs/Kconfig"' "$src/fs/Kconfig" ||
    sed -i 's|^source "fs/fuse/Kconfig"|source "fs/fxfs/Kconfig"\n&|' "$src/fs/Kconfig"
grep -q 'CONFIG_FXFS_FS' "$src/fs/Makefile" ||
    echo 'obj-$(CONFIG_FXFS_FS)		+= fxfs/' >> "$src/fs/Makefile"
"$src/scripts/config" --file "$src/.config" --enable FXFS_FS

# drivers/virtio_extent, the extent-store device's driver (TASK-26.5), and its header.
rsync -a --delete --exclude '*.o' --exclude '.*.cmd' \
    "$root/guest/linux/drivers/virtio_extent/" "$src/drivers/virtio_extent/"
cp "$root/guest/linux/include/linux/virtio_extent.h" "$src/include/linux/"
grep -q '^source "drivers/virtio_extent/Kconfig"' "$src/drivers/Kconfig" ||
    sed -i 's|^source "drivers/virtio/Kconfig"|&\nsource "drivers/virtio_extent/Kconfig"|' \
        "$src/drivers/Kconfig"
grep -q 'CONFIG_VIRTIO_EXTENT' "$src/drivers/Makefile" ||
    echo 'obj-$(CONFIG_VIRTIO_EXTENT)	+= virtio_extent/' >> "$src/drivers/Makefile"
"$src/scripts/config" --file "$src/.config" --enable VIRTIO_EXTENT
# virtio-pmem, for the benchmark's ext4 with DAX on writable pmem, which the guest's Fxfs compares
# with.
"$src/scripts/config" --file "$src/.config" --enable VIRTIO_PMEM
# sysrq, through /proc/sysrq-trigger only, whose blocked tasks' stacks (w) the bench's watchdog
# dumps when a workload hangs.
"$src/scripts/config" --file "$src/.config" --enable MAGIC_SYSRQ --set-val MAGIC_SYSRQ_DEFAULT_ENABLE 0
# A console on Firecracker's serial port, an 8250 UART (on arm64, one the device tree describes).
"$src/scripts/config" --file "$src/.config" --enable SERIAL_8250 --enable SERIAL_8250_CONSOLE \
    --enable SERIAL_OF_PLATFORM
# With FXDAX_GUEST_KERNEL_PROFILE=1, an x86-64 kernel unwinds its stacks by frame pointers, which
# Fxfs's Rust keeps, so that perf's call graphs follow calls through it: libkrunfw's guesses.
# Frame pointers cost the kernel's own code a little, so a kernel measured has none: build one
# for profiling with its own FXDAX_GUEST_KERNEL_WORK.
if [[ $arch == x86_64 && ${FXDAX_GUEST_KERNEL_PROFILE:-} == 1 ]]; then
    "$src/scripts/config" --file "$src/.config" --disable UNWINDER_GUESS \
        --enable UNWINDER_FRAME_POINTER
fi
make -C "$src" "${make_args[@]}" olddefconfig
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
archive="$src/fs/fxfs/libfxfs_linux.a"
nm -A --defined-only "$archive" 2>/dev/null |
    awk '$NF ~ /^(memcpy|memmove|memset|memcmp|bcmp|strlen)$/ { split($1, f, ":"); print f[2] }' |
    sort -u | while read -r member; do ar d "$archive" "$member"; done

make -C "$src" "${make_args[@]}" -j"$(nproc)" KBUILD_BUILD_USER=root KBUILD_BUILD_HOST=fxdax \
    "$make_target"
# The kernel's own memcpy, which Rust calls, is in the kernel: not Rust's.
! nm "$src/fs/fxfs/fxfs_rust.o" | grep -qE ' [TtWw] (memcpy|memmove|memset|memcmp)$'
out="$work/out/$arch"
mkdir -p "$out"
cp "$src/$image" "$out/"
echo "$out/$(basename "$image")"
