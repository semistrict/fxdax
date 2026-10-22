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
# case's, and drops the passing rounds'. FXDAX_FIRECRACKER_SECCOMP (real, log) runs
# Firecracker under its production seccomp filter, and FXDAX_FIRECRACKER_JAIL=1 in its jailer's
# jail, as an unprivileged user (see below).
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
    tests=(extent-guest fxfs-guest fxfs-guest-dax fxfs-guest-checks fxfs-guest-checks-dax fxfs-guest-pgbench-dax)
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

# Under the bench's perf (FXDAX_BENCH_PERF), with frame pointers, which its call graphs walk, in a
# target of its own, as the flags differ.
extentd_target="$root/target"
if [[ -n ${FXDAX_BENCH_PERF:-} ]]; then
    extentd_target="$root/target/frame-pointers"
    RUSTFLAGS="-C force-frame-pointers=yes" CARGO_TARGET_DIR="$extentd_target" \
        cargo build --release --manifest-path "$root/Cargo.toml" -p fxdax --bin fxdax-extentd
else
    cargo build --release --manifest-path "$root/Cargo.toml" -p fxdax --bin fxdax-extentd
fi
extentd_bin="$extentd_target/release/fxdax-extentd"
# From its directory, whose .cargo/config.toml sets its target directory and build environment.
# For the host's target, Firecracker has no seccomp filter of its own, and allows every system
# call. FXDAX_FIRECRACKER_SECCOMP builds it as production does, for musl, which compiles in its
# filter (TASK-50.1): `real` runs under that; `log` under a copy of it that allows what it would
# refuse, logging it, which $results/seccomp-log.txt lists afterwards, by thread and system call.
firecracker_dir="$root/vendor/firecracker"
seccomp=${FXDAX_FIRECRACKER_SECCOMP:-}
case "$seccomp" in
    "")
        (cd "$firecracker_dir" && cargo build --release -p firecracker --features fxdax)
        export FXDAX_FIRECRACKER="$firecracker_dir/build/cargo_target/release/firecracker"
        ;;
    real | log)
        musl="$(uname -m)-unknown-linux-musl"
        # musl-gcc's headers lack the kernel's, which userfaultfd-sys includes: the host's, after
        # musl's own (Firecracker's container has them in musl's tree).
        musl_cflags="CFLAGS_${musl//-/_}=-idirafter /usr/include -idirafter /usr/include/$(uname -m)-linux-gnu"
        (cd "$firecracker_dir" &&
            env "$musl_cflags" cargo build --release -p firecracker --features fxdax --target "$musl")
        export FXDAX_FIRECRACKER="$firecracker_dir/build/cargo_target/$musl/release/firecracker"
        if [[ $seccomp == log ]]; then
            python3 - "$firecracker_dir/resources/seccomp/$musl.json" "$results/seccomp-log.json" <<'PY'
import json, sys
filters = json.load(open(sys.argv[1]))
for thread in filters.values():
    thread["default_action"] = "log"
json.dump(filters, open(sys.argv[2], "w"), indent=1)
PY
            (cd "$firecracker_dir" && cargo run --release -q -p seccompiler --bin seccompiler-bin -- \
                --target-arch "$(uname -m)" --input-file "$results/seccomp-log.json" \
                --output-file "$results/seccomp-log.bpf")
            export FXDAX_FIRECRACKER_SECCOMP_FILTER="$results/seccomp-log.bpf"
            # Every refusal it logs, not the first few of each burst.
            sysctl -q kernel.printk_ratelimit=0
            dmesg --clear
        fi
        ;;
    *) echo "FXDAX_FIRECRACKER_SECCOMP is real or log" >&2; exit 2 ;;
esac
# FXDAX_FIRECRACKER_JAIL=1 runs each Firecracker as production does (TASK-50.1, TASK-50.4): in
# the jail Firecracker's jailer makes, as an unprivileged user (64000), with /dev/userfaultfd in
# the jail and vm.unprivileged_userfaultfd off; with FXDAX_FIRECRACKER_SECCOMP set, for musl.
if [[ -n ${FXDAX_FIRECRACKER_JAIL:-} ]]; then
    [[ -n $seccomp ]] || { echo "FXDAX_FIRECRACKER_JAIL needs FXDAX_FIRECRACKER_SECCOMP" >&2; exit 2; }
    (cd "$firecracker_dir" && env "$musl_cflags" cargo build --release -p jailer --target "$musl")
    export FXDAX_FIRECRACKER_JAILER="$firecracker_dir/build/cargo_target/$musl/release/jailer"
    export FXDAX_JAIL_UID=64000 FXDAX_JAIL_GID=64000
    sysctl -q vm.unprivileged_userfaultfd=0
    # What the daemons and the tests make, sockets and images, the jailed user reaches.
    umask 000
fi
# fxfs-guest-docker-dax's Docker, its static binaries (Apache-2.0), downloaded once, and BusyBox,
# its containers' image, the host's static one (busybox-static).
if [[ -z ${FXDAX_DOCKER_STATIC:-} && " ${tests[*]} " =~ \ (all|fxfs-guest-docker-dax)\  ]]; then
    docker_version=29.9.1
    case $(uname -m) in
        x86_64) docker_sha256=c158ecc55adb3afb01bbd3539ed3b997d20c26dbdc418080506cc1bf5243ff57 ;;
        aarch64) docker_sha256=3e93cc6d1a686456a3b18eefdde26467a837a7a2b257af22e27cba1dbc1b2018 ;;
    esac
    docker_dir="${XDG_CACHE_HOME:-$HOME/.cache}/fxdax/docker-$docker_version"
    if [[ ! -x $docker_dir/dockerd ]]; then
        mkdir -p "$docker_dir"
        curl -fsSL -o "$docker_dir.tgz" \
            "https://download.docker.com/linux/static/stable/$(uname -m)/docker-$docker_version.tgz"
        echo "$docker_sha256  $docker_dir.tgz" | sha256sum --check --quiet
        tar -xzf "$docker_dir.tgz" -C "$docker_dir" --strip-components=1
        rm "$docker_dir.tgz"
    fi
    export FXDAX_DOCKER_STATIC="$docker_dir" FXDAX_BUSYBOX="${FXDAX_BUSYBOX:-/bin/busybox}"
fi
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
    "$extentd_bin" "$socket" > "$results/extentd.log" 2>&1 &
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
export FXDAX_EXTENTD_BIN="$extentd_bin"

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
if [[ $seccomp == log ]]; then
    # The audit records of the system calls the filter would refuse: thread, call, count.
    dmesg | python3 -c '
import collections, re, sys
seen = collections.Counter()
for line in sys.stdin:
    if "type=1326" not in line:
        continue
    # A name with a space, as vCPU threads have, is hexadecimal, unquoted.
    comm = re.search(r"comm=(?:\"([^\"]*)\"|([0-9A-F]+)\b)", line)
    call = re.search(r"syscall=(\d+)", line)
    if comm and call:
        name = comm.group(1) or bytes.fromhex(comm.group(2)).decode(errors="replace")
        seen[(name, int(call.group(1)))] += 1
for (comm, call), count in sorted(seen.items()):
    print(f"{comm}\t{call}\t{count}")
' > "$results/seccomp-log.txt"
    echo "seccomp: $(wc -l < "$results/seccomp-log.txt") thread and call pairs refused" >&2
fi
echo "Logs: $results" >&2
exit "$status"
