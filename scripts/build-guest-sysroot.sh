#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build a sysroot for one of the guest kernel's targets, holding core, alloc and guest/rust/std as
# std (TASK-26.1, TASK-26.6), so that crates written for std built against it use the kernel's.
#
#   build-guest-sysroot.sh TARGET DIR
#
# leaves the sysroot in DIR/sysroot, and std's build in DIR/std-target, and prints the rustflags
# that build for TARGET against it, as the kernel builds its own Rust. Everything else of the
# kernel's Rust builds with them: guest/rust's crates, and Fxfs.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:?target required}
work=${2:?directory required}
mkdir -p "$work"
work=$(cd "$work" && pwd)

# As the kernel builds its own Rust: no tables to unwind with, and code in one section per object,
# which link-time optimization makes the filesystem's one object; on arm64 no NEON, and on x86-64
# linked at kernel addresses, with the kernel's code model and return thunks.
flags="-Cforce-unwind-tables=n -Zfunction-sections=n"
# Frame pointers, as the kernel's own code keeps, so that perf follows calls through Rust.
flags+=" -Cforce-frame-pointers=yes"
case "$target" in
    aarch64-unknown-none-softfloat) flags+=" -Ctarget-feature=-neon" ;;
    x86_64-unknown-none)
        flags+=" -Crelocation-model=static -Ccode-model=kernel"
        flags+=" -Zfunction-return=thunk-extern -Zretpoline-external-thunk"
        ;;
    *) echo "Unsupported target: $target" >&2; exit 2 ;;
esac

# Each library's metadata, and the rlib beside it, in place of those of a std built before.
lib="$work/sysroot/lib/rustlib/$target/lib"
mkdir -p "$lib"
rm -f "$lib"/*.rlib "$lib"/*.rmeta
# core and alloc built for the target, without compiler_builtins' memcpy and friends, which the
# kernel has, and with the compiler's intrinsics an object apiece, as Rust's own builds have them,
# so that the kernel links only those it calls.
(cd "$root/guest/rust" && RUSTFLAGS="$flags" CARGO_TARGET_DIR="$work/std-target" \
    cargo build -q -Zbuild-std=core,alloc -p linux-std --release --target "$target" \
    --config 'profile.release.package.compiler_builtins.codegen-units=10000' \
    --message-format=json > "$work/std.json")
jq -r 'select(.reason == "compiler-message") | .message.rendered' "$work/std.json" >&2
jq -r 'select(.reason == "compiler-artifact") | .filenames[] | select(endswith(".rmeta"))' \
    "$work/std.json" | while read -r rmeta; do cp "$rmeta" "${rmeta%.rmeta}.rlib" "$lib/"; done

# getrandom has no backend for the kernel: the kernel's Rust provides one. Cargo doesn't see the
# sysroot change, but it sees the flags do, and rebuilds against a new std: they name its hash.
std=$(cat "$lib"/*.rlib | shasum | cut -c1-16)
echo "--sysroot $work/sysroot $flags --cfg getrandom_backend=\"custom\" --cfg guest_std=\"$std\""
