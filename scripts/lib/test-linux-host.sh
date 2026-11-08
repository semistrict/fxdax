#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on an x86-64 Linux KVM host, as root. It runs the
# workspace's tests (unless FXDAX_WORKSPACE_TESTS=0; FXDAX_TEST_ARGS, split at
# spaces, are cargo nextest's arguments, such as a filter), then fxdax's guest
# tests (unless FXDAX_GUEST_TESTS=0; scripts/test-e2e.sh; TESTs name them),
# whose DAX window takes KVM's faults through userfaultfd. With FXDAX_HOST_TRACE, a bpftrace
# script of SOURCE's traces the host as the guest tests run, its maps in RESULTS/host-trace.txt
# (scripts/extent-transport.bt, TASK-164.11.1). With FXDAX_GUEST_KERNEL_BUILD set
# (release; debug: lockdep, KCSAN, DEBUG_ATOMIC_SLEEP; kasan: KASAN, KFENCE, lockdep; or coverage:
# gcov of fs/fxfs, which leaves coverage/ in the results, scripts/guest-coverage.py), the guest
# tests run on a guest kernel it builds from the source (scripts/build-guest-kernel.sh), not
# FXDAX_GUEST_KERNEL's; with neither set, on a release one it builds (TASK-130: without one, every
# guest case skipped). It builds in trees that persist between runs, so that a run builds only
# what changed.
#
# Usage: test-linux-host.sh SOURCE RESULTS [TEST...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
shift 2
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
host_tree test-tree
host_need cargo-nextest

status=0
host_record > "$results/host.txt"

# scripts/test.sh's parts but the Go client's, which CI's go job runs: these hosts have no Go
# (go.mod wants 1.26, newer than Ubuntu's), and the part, added by TASK-219, failed every run here
# with exit 127, the parts after it unrun (2026-11-07, TASK-222's run).
export FXDAX_TEST_PARTS=${FXDAX_TEST_PARTS:-clippy workspace sim shuttle scripts guest firecracker}

if [[ ${FXDAX_WORKSPACE_TESTS:-1} != 0 ]]; then
    echo "== fxdax tests" >&2
    read -ra test_args <<< "${FXDAX_TEST_ARGS:-}"
    "$repo/scripts/test.sh" --no-fail-fast "${test_args[@]}" \
        > "$results/fxdax-tests.log" 2>&1 || status=$?
    echo "fxdax tests: exit $status" >&2
fi

# The pjdfstest (BSD-2-Clause) commit the pjdfstest cases build.
source "$(dirname "${BASH_SOURCE[0]}")/pjdfstest.sh"

if [[ ${FXDAX_GUEST_TESTS:-1} == 0 ]]; then
    exit "$status"
fi
if [[ -z ${FXDAX_GUEST_KERNEL:-} ]]; then
    FXDAX_GUEST_KERNEL_BUILD=${FXDAX_GUEST_KERNEL_BUILD:-release}
fi
case ${FXDAX_GUEST_KERNEL_BUILD:-} in
    '') ;;
    release | debug | kasan | coverage | rust-checks)
        echo "== guest kernel ($FXDAX_GUEST_KERNEL_BUILD)" >&2
        # Each kind in a work directory of its own, as their configurations differ. The kernel's
        # Rust builds on guest/rust's own toolchain, not the host's.
        kernel_work="$work/guest-kernel-$FXDAX_GUEST_KERNEL_BUILD"
        kernel_debug=0 kernel_kasan=0 kernel_coverage=0 kernel_rust_checks=0
        [[ $FXDAX_GUEST_KERNEL_BUILD == debug ]] && kernel_debug=1
        [[ $FXDAX_GUEST_KERNEL_BUILD == kasan ]] && kernel_kasan=1
        [[ $FXDAX_GUEST_KERNEL_BUILD == coverage ]] && kernel_coverage=1
        [[ $FXDAX_GUEST_KERNEL_BUILD == rust-checks ]] && kernel_rust_checks=1
        channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$repo/guest/rust/rust-toolchain.toml")
        rustup toolchain install "$channel" --component rust-src --profile minimal > /dev/null
        if ! env -u RUSTUP_TOOLCHAIN FXDAX_GUEST_KERNEL_WORK="$kernel_work" \
            FXDAX_GUEST_KERNEL_DEBUG=$kernel_debug FXDAX_GUEST_KERNEL_KASAN=$kernel_kasan \
            FXDAX_GUEST_KERNEL_COVERAGE=$kernel_coverage \
            FXDAX_GUEST_KERNEL_RUST_CHECKS=$kernel_rust_checks \
            "$repo/scripts/build-guest-kernel.sh" x86_64 \
            > "$results/guest-kernel.log" 2>&1; then
            echo "guest kernel: failed, see guest-kernel.log" >&2
            exit 1
        fi
        export FXDAX_GUEST_KERNEL="$kernel_work/out/x86_64/vmlinux"
        # Its kind, which the guest tests' time limits scale by (TASK-168).
        export FXDAX_GUEST_KERNEL_BUILD
        sha256sum "$FXDAX_GUEST_KERNEL" > "$results/guest-kernel.txt"
        ;;
    *)
        echo "FXDAX_GUEST_KERNEL_BUILD is release, debug, kasan, coverage or rust-checks" >&2
        exit 2
        ;;
esac
if [[ ${FXDAX_GUEST_KERNEL_BUILD:-} == coverage ]]; then
    export FXDAX_COVERAGE="$results/coverage-counts"
    mkdir -p "$FXDAX_COVERAGE"
fi
# A crash leaves a core, whose threads' stacks the results keep, and only those, as the host goes
# with the core: a VMM died of SIGSEGV in a soak, and nothing said where (2026-10-31). Private
# memory alone (coredump_filter): a guest's and a daemon's arenas are shared.
cores="$results/cores"
mkdir -p "$cores"
sysctl -q -w kernel.core_pattern="$cores/core.%p"
ulimit -c unlimited
echo 0x11 > /proc/self/coredump_filter
echo "== guest tests" >&2
# With FXDAX_RESULTS_BUCKET, a prefix of a bucket the run's results go to as each case ends, as they
# are here (scripts/guest_results.py), by gcloud, as the host's service account.
if [[ -n ${FXDAX_RESULTS_BUCKET:-} ]]; then
    command -v gcloud > /dev/null || snap install google-cloud-cli --classic > /dev/null ||
        echo "no gcloud: the guest tests' results stay here alone" >&2
fi
tracer=
if [[ -n ${FXDAX_HOST_TRACE:-} ]]; then
    host_need bpftrace
    # What it says, its first MiB, the rest read and dropped, which a closed pipe would kill it
    # for: extent-transport.bt had it say 700 MB over a 30-minute case (GCE r810, r811).
    bpftrace -q -o "$results/host-trace.txt" "$repo/$FXDAX_HOST_TRACE" \
        2> >({ head -c 1048576; cat > /dev/null; } > "$results/host-trace.log") &
    tracer=$!
fi
e2e_status=0
FXDAX_E2E_RESULTS="$results/e2e" \
    FXDAX_RESULTS_BUCKET="${FXDAX_RESULTS_BUCKET:+${FXDAX_RESULTS_BUCKET%/}/e2e}" \
    "$repo/scripts/test-e2e.sh" "$@" > "$results/e2e.log" 2>&1 || e2e_status=$?
echo "guest tests: exit $e2e_status" >&2
# The tracer prints its maps as it is interrupted.
if [[ -n $tracer ]]; then
    kill -INT "$tracer"
    wait "$tracer" || echo "host trace: exit $? ($results/host-trace.log)" >&2
fi
for core in "$cores"/core.*; do
    [[ -f $core ]] || continue
    echo "guest tests: a process crashed: $core.stacks.txt" >&2
    eu-stack -l -i -n 512 --core="$core" > "$core.stacks.txt" 2>&1 || true
    rm "$core"
done
if [[ -n ${FXDAX_COVERAGE:-} ]]; then
    "$repo/scripts/guest-coverage.py" "$FXDAX_COVERAGE" "$(dirname "$(find "$kernel_work" \
        -path '*/fs/fxfs/dax.gcno' -print -quit)")" "$results/coverage" ||
        echo "guest coverage: failed" >&2
fi
if (( status == 0 )); then status=$e2e_status; fi
exit "$status"
