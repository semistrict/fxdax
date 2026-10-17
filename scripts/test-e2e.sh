#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run libkrun's fxdax guest tests on this host, Linux with KVM (on macOS, use
# scripts/test-e2e-linux.sh, which runs this in Lima). Arguments name the test
# cases (default: extent-guest, fxfs-guest, fxfs-guest-dax, fxfs-guest-checks
# and fxfs-guest-checks-dax, which boot the guest kernel FXDAX_GUEST_KERNEL
# names: scripts/build-guest-kernel.sh builds it). Each test's logs are kept
# under $FXDAX_E2E_RESULTS (default: a new temporary directory).
#
# The guests' extent stores are served by fxdax's extent daemon, which this
# starts, as FXDAX_EXTENTD tells the tests. LIBCLANG_PATH names libclang's
# directory where llvm-config cannot, KRUNFW_LIB_DIR the directory holding
# libkrunfw.so.5 where the loader cannot find it, and DAX needs access to
# /dev/userfaultfd (see the README).
#
# With FXDAX_VMM=firecracker, the guests boot in fxdax's Firecracker
# (vendor/firecracker, with its fxdax feature), which this builds, rather than
# in libkrun: the tests that need libkrun's own devices or kernel skip.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
libkrun="$root/vendor/libkrun"
tests=("$@")
if (( ${#tests[@]} == 0 )); then
    tests=(extent-guest fxfs-guest fxfs-guest-dax fxfs-guest-checks fxfs-guest-checks-dax)
fi
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)
features=(FXDAX=1 BLK=1 NET=1)
extra=()

if [[ $(uname -s) != Linux ]]; then
    echo "fxdax's guests run on Linux only: use scripts/test-e2e-linux.sh" >&2
    exit 2
fi
if [[ -z ${LIBCLANG_PATH:-} ]] && command -v llvm-config > /dev/null; then
    LIBCLANG_PATH=$(llvm-config --libdir)
fi
if [[ -n ${LIBCLANG_PATH:-} ]]; then export LIBCLANG_PATH; fi
if [[ -n ${KRUNFW_LIB_DIR:-} ]]; then extra+=(EXTRA_LIBPATH_Linux="$KRUNFW_LIB_DIR"); fi

cargo build --release --manifest-path "$root/Cargo.toml" -p fxdax --bin fxdax-extentd
case ${FXDAX_VMM:-libkrun} in
    libkrun) ;;
    firecracker)
        # From its directory, whose .cargo/config.toml sets its target directory and build
        # environment.
        (cd "$root/vendor/firecracker" && cargo build --release -p firecracker --features fxdax)
        export FXDAX_VMM \
            FXDAX_FIRECRACKER="$root/vendor/firecracker/build/cargo_target/release/firecracker"
        ;;
    *) echo "FXDAX_VMM is libkrun or firecracker" >&2; exit 2 ;;
esac
socket="$results/extentd.sock"
if [[ -e $socket ]]; then
    echo "$socket exists: a daemon may be running" >&2
    exit 2
fi
# What it did with object storage, if it keeps stores there (FXDAX_EXTENTD_REMOTE), which
# scripts/bench-report.py reports.
FXDAX_EXTENTD_STATS="$results/extentd-stats.txt" \
    "$root/target/release/fxdax-extentd" "$socket" > "$results/extentd.log" 2>&1 &
daemon=$!
trap 'kill "$daemon" 2> /dev/null; rm -f "$socket"' EXIT
while [[ ! -S $socket ]]; do
    if ! kill -0 "$daemon" 2> /dev/null; then
        echo "the extent daemon exited: see $results/extentd.log" >&2
        exit 1
    fi
    sleep 0.1
done
export FXDAX_EXTENTD="$socket"
# fxdax: the daemon's binary, for cases that start daemons of their own.
export FXDAX_EXTENTD_BIN="$root/target/release/fxdax-extentd"

status=0
for test in "${tests[@]}"; do
    make -C "$libkrun" test "${features[@]}" ${extra[@]+"${extra[@]}"} TEST="$test" \
        TEST_FLAGS="--base-dir $results/$test --keep-all" || status=1
done
echo "Logs: $results" >&2
exit "$status"
