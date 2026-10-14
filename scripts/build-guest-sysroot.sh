#!/usr/bin/env bash
# Build a sysroot for one of the guest kernel's targets, holding core, alloc and guest/rust/std as
# std (TASK-26.1, TASK-26.6), so that crates written for std built against it use the kernel's.
#
#   build-guest-sysroot.sh TARGET DIR
#
# leaves the sysroot in DIR/sysroot, and std's build in DIR/std-target, and prints the rustflags
# that build for TARGET against it, as guest/rust/.cargo/config.toml builds for the target.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:?target required}
work=${2:?directory required}
mkdir -p "$work"
work=$(cd "$work" && pwd)

case "$target" in
    aarch64-unknown-none-softfloat) flags="-Cforce-unwind-tables=n -Ctarget-feature=-neon" ;;
    x86_64-unknown-none)
        flags="-Cforce-unwind-tables=n -Crelocation-model=static -Ccode-model=kernel"
        flags+=" -Zfunction-return=thunk-extern -Zretpoline-external-thunk"
        ;;
    *) echo "Unsupported target: $target" >&2; exit 2 ;;
esac

# Each library's metadata, and the rlib beside it, in place of those of a std built before.
lib="$work/sysroot/lib/rustlib/$target/lib"
mkdir -p "$lib"
rm -f "$lib"/*.rlib "$lib"/*.rmeta
(cd "$root/guest/rust" && CARGO_TARGET_DIR="$work/std-target" \
    cargo build -q -p linux-std --release --target "$target" --message-format=json \
    > "$work/std.json")
jq -r 'select(.reason == "compiler-artifact") | .filenames[] | select(endswith(".rmeta"))' \
    "$work/std.json" | while read -r rmeta; do cp "$rmeta" "${rmeta%.rmeta}.rlib" "$lib/"; done

# getrandom has no backend for the kernel: the kernel's Rust provides one. Cargo doesn't see the
# sysroot change, but it sees the flags do, and rebuilds against a new std: they name its hash.
std=$(cat "$lib"/*.rlib | shasum | cut -c1-16)
echo "--sysroot $work/sysroot $flags --cfg getrandom_backend=\"custom\" --cfg guest_std=\"$std\""
