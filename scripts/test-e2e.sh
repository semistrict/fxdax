#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run fxdax's guest tests (guest/tests) on this host, Linux with KVM (on macOS, use
# scripts/test-e2e-linux.sh, which runs this in Lima). Arguments name the test
# cases (default: extent-guest, fxfs-guest, fxfs-guest-dax, fxfs-guest-checks
# and fxfs-guest-checks-dax), which boot fxdax's Firecracker (vendor/firecracker,
# with its fxdax feature), which this builds, and the guest kernel
# FXDAX_GUEST_KERNEL names (scripts/build-guest-kernel.sh builds it). Each test's
# logs are kept under $FXDAX_E2E_RESULTS (default: a new temporary directory).
# FXDAX_E2E_ROUNDS runs each case that many times (default 1), as a soak: it stops
# at a case's first failing round, whose logs it keeps, in ROUND-K under the
# case's, and drops the passing rounds'.
#
# The guests' extent stores are served by fxdax's extent daemon, which this
# starts, as FXDAX_EXTENTD tells the tests; its control API is beside it, at
# extentd.control. LIBCLANG_PATH names libclang's
# directory where llvm-config cannot, and DAX needs access to /dev/userfaultfd
# (see the README).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
harness="$root/guest/tests"
tests=("$@")
if (( ${#tests[@]} == 0 )); then
    tests=(extent-guest fxfs-guest fxfs-guest-dax fxfs-guest-checks fxfs-guest-checks-dax)
fi
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)

if [[ $(uname -s) != Linux ]]; then
    echo "fxdax's guests run on Linux only: use scripts/test-e2e-linux.sh" >&2
    exit 2
fi
if [[ -z ${LIBCLANG_PATH:-} ]] && command -v llvm-config > /dev/null; then
    LIBCLANG_PATH=$(llvm-config --libdir)
fi
if [[ -n ${LIBCLANG_PATH:-} ]]; then export LIBCLANG_PATH; fi

cargo build --release --manifest-path "$root/Cargo.toml" -p fxdax --bin fxdax-extentd
# From its directory, whose .cargo/config.toml sets its target directory and build environment.
(cd "$root/vendor/firecracker" && cargo build --release -p firecracker --features fxdax)
export FXDAX_FIRECRACKER="$root/vendor/firecracker/build/cargo_target/release/firecracker"
# The harness: the guest agent, a static binary each guest runs as its init, and the runner,
# which boots a VM for each test. pjdfstest's cases take its repository and commit as they build.
guest_target="$(uname -m)-unknown-linux-musl"
(cd "$harness" && cargo test -q -p test_cases --features guest)
(cd "$harness" && cargo build -q --target "$guest_target" -p guest-agent)
(cd "$harness" && cargo build -q -p runner)
export FXDAX_GUEST_AGENT="$harness/target/$guest_target/debug/guest-agent"
socket="$results/extentd.sock"
control="$results/extentd.control"
if [[ -e $socket || -e $control ]]; then
    echo "$socket exists: a daemon may be running" >&2
    exit 2
fi
# A host keeps stores in a bucket, FXDAX_EXTENTD_REMOTE's or one in its memory, as of placement 1
# unless one is named; what it did with it scripts/bench-report.py reports.
env FXDAX_EXTENTD_REMOTE="${FXDAX_EXTENTD_REMOTE:-memory:///e2e}" \
    FXDAX_EXTENTD_TOKEN="${FXDAX_EXTENTD_TOKEN:-1}" \
    FXDAX_EXTENTD_STATS="$results/extentd-stats.txt" FXDAX_EXTENTD_GRPC="$control" \
    "$root/target/release/fxdax-extentd" "$socket" > "$results/extentd.log" 2>&1 &
daemon=$!
trap 'kill "$daemon" 2> /dev/null; rm -f "$socket" "$control"' EXIT
while [[ ! -S $socket || ! -S $control ]]; do
    if ! kill -0 "$daemon" 2> /dev/null; then
        echo "the extent daemon exited: see $results/extentd.log" >&2
        exit 1
    fi
    sleep 0.1
done
export FXDAX_EXTENTD="$socket"
# fxdax: the daemon's binary, for cases that start daemons of their own.
export FXDAX_EXTENTD_BIN="$root/target/release/fxdax-extentd"

rounds=${FXDAX_E2E_ROUNDS:-1}
[[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "FXDAX_E2E_ROUNDS is a count" >&2; exit 2; }
status=0
for test in "${tests[@]}"; do
    if (( rounds == 1 )); then
        "$harness/target/debug/runner" test --test-case "$test" --base-dir "$results/$test" \
            --keep-all || status=1
        continue
    fi
    for (( round = 1; round <= rounds; round++ )); do
        base="$results/$test/round-$round"
        if ! "$harness/target/debug/runner" test --test-case "$test" --base-dir "$base" \
            --keep-all; then
            echo "$test: failed in round $round of $rounds: see $base" >&2
            status=1
            break
        fi
        rm -r "$base"
        if (( round == rounds )); then
            echo "$test: $rounds rounds passed" >&2
        fi
    done
done
echo "Logs: $results" >&2
exit "$status"
