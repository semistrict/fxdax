#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Qualify fxdax on a disposable x86-64 Linux KVM host, as root. It runs the
# workspace's tests (unless FXDAX_WORKSPACE_TESTS=0; FXDAX_TEST_ARGS, split at
# spaces, are cargo nextest's arguments, such as a filter), then fxdax's guest
# tests (unless FXDAX_GUEST_TESTS=0; scripts/test-e2e.sh; TESTs name them),
# whose DAX window takes KVM's faults through userfaultfd. It builds in a tree that persists between runs,
# so that a run builds only what changed.
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
echo "== guest tests" >&2
e2e_status=0
FXDAX_E2E_RESULTS="$results/e2e" "$repo/scripts/test-e2e.sh" "$@" \
    > "$results/e2e.log" 2>&1 || e2e_status=$?
echo "guest tests: exit $e2e_status" >&2
if (( status == 0 )); then status=$e2e_status; fi
exit "$status"
