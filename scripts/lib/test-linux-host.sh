#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on a disposable x86-64 Linux KVM host, as root. It runs the
# workspace's tests (unless FXDAX_WORKSPACE_TESTS=0; FXDAX_TEST_ARGS, split at
# spaces, are cargo nextest's arguments, such as a filter), then fxdax's guest
# tests (unless FXDAX_GUEST_TESTS=0; scripts/test-e2e.sh; TESTs name them),
# whose DAX window takes KVM's faults through userfaultfd. With FXDAX_GUEST_KERNEL_BUILD set
# (release; debug: lockdep, KCSAN, DEBUG_ATOMIC_SLEEP; or kasan: KASAN, KFENCE, lockdep), the guest
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

if [[ ${FXDAX_WORKSPACE_TESTS:-1} != 0 ]]; then
    echo "== fxdax tests" >&2
    read -ra test_args <<< "${FXDAX_TEST_ARGS:-}"
    "$repo/scripts/test.sh" --no-fail-fast "${test_args[@]}" \
        > "$results/fxdax-tests.log" 2>&1 || status=$?
    echo "fxdax tests: exit $status" >&2
fi

# The pjdfstest (BSD-2-Clause) commit the pjdfstest cases build (as scripts/test-guest-kernel.sh).
export PJDFSTEST_REPO=${PJDFSTEST_REPO:-https://github.com/pjd/pjdfstest}
export PJDFSTEST_COMMIT=${PJDFSTEST_COMMIT:-85a8aea9e685999ef0540392fd80535f873d7ff7}

if [[ ${FXDAX_GUEST_TESTS:-1} == 0 ]]; then
    exit "$status"
fi
if [[ -z ${FXDAX_GUEST_KERNEL:-} ]]; then
    FXDAX_GUEST_KERNEL_BUILD=${FXDAX_GUEST_KERNEL_BUILD:-release}
fi
case ${FXDAX_GUEST_KERNEL_BUILD:-} in
    '') ;;
    release | debug | kasan)
        echo "== guest kernel ($FXDAX_GUEST_KERNEL_BUILD)" >&2
        # Each kind in a work directory of its own, as their configurations differ. The kernel's
        # Rust builds on guest/rust's own toolchain, not the host's.
        kernel_work="$work/guest-kernel-$FXDAX_GUEST_KERNEL_BUILD"
        kernel_debug=0 kernel_kasan=0
        [[ $FXDAX_GUEST_KERNEL_BUILD == debug ]] && kernel_debug=1
        [[ $FXDAX_GUEST_KERNEL_BUILD == kasan ]] && kernel_kasan=1
        channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$repo/guest/rust/rust-toolchain.toml")
        rustup toolchain install "$channel" --component rust-src --profile minimal > /dev/null
        if ! env -u RUSTUP_TOOLCHAIN FXDAX_GUEST_KERNEL_WORK="$kernel_work" \
            FXDAX_GUEST_KERNEL_DEBUG=$kernel_debug FXDAX_GUEST_KERNEL_KASAN=$kernel_kasan \
            "$repo/scripts/build-guest-kernel.sh" x86_64 \
            > "$results/guest-kernel.log" 2>&1; then
            echo "guest kernel: failed, see guest-kernel.log" >&2
            exit 1
        fi
        export FXDAX_GUEST_KERNEL="$kernel_work/out/x86_64/vmlinux"
        sha256sum "$FXDAX_GUEST_KERNEL" > "$results/guest-kernel.txt"
        ;;
    *) echo "FXDAX_GUEST_KERNEL_BUILD is release, debug or kasan" >&2; exit 2 ;;
esac
echo "== guest tests" >&2
e2e_status=0
FXDAX_E2E_RESULTS="$results/e2e" "$repo/scripts/test-e2e.sh" "$@" \
    > "$results/e2e.log" 2>&1 || e2e_status=$?
echo "guest tests: exit $e2e_status" >&2
if (( status == 0 )); then status=$e2e_status; fi
exit "$status"
