#!/usr/bin/env bash
# Build Fxfs's third-party dependencies (guest/std-probe) for one of the kernel's targets, with
# guest/rust/std as their std (TASK-26.1), and list the crates that do not build.
#
#   probe-guest-std.sh [aarch64-unknown-none-softfloat|x86_64-unknown-none]
#
# std, core and alloc are built for the target and laid out as a sysroot, which the probe is
# built against, so that crates written for std use the kernel's. Everything lives in
# $FXDAX_GUEST_STD_WORK (default: a new temporary directory), the build's log in probe.log. Needs
# jq.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:-aarch64-unknown-none-softfloat}
work=${FXDAX_GUEST_STD_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-std.XXXXXX")}
mkdir -p "$work"
work=$(cd "$work" && pwd)
toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/guest/rust/rust-toolchain.toml")

rustflags=$("$root/scripts/build-guest-sysroot.sh" "$target" "$work")

status=0
(cd "$root/guest/std-probe" &&
    RUSTFLAGS="$rustflags" \
    CARGO_TARGET_DIR="$work/probe-target" \
    cargo "+$toolchain" build --keep-going --release --target "$target" \
    > "$work/probe.log" 2>&1) || status=$?
grep '^error: could not compile' "$work/probe.log" | sed 's/^error: could not compile /Fails: /' || true
echo "Log: $work/probe.log" >&2
exit "$status"
