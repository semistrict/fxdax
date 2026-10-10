#!/usr/bin/env bash
# Qualify fxdax on a disposable x86-64 Linux KVM host, as root. It runs the
# workspace's tests, then libkrun's guest tests (scripts/test-e2e.sh; TESTs
# name them), whose DAX window takes KVM's faults through userfaultfd.
#
# Usage: test-linux-host.sh SOURCE RESULTS [TEST...]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
shift 2
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
if [[ "$(cargo nextest --version 2>/dev/null | head -1)" != 'cargo-nextest 0.9.143' ]]; then
    cargo install cargo-nextest --version 0.9.143 --locked
fi

status=0
{
    uname -a
    rustc --version
    cargo nextest --version | head -1
} > "$results/host.txt"

echo "== fxdax tests" >&2
CARGO_TARGET_DIR="$work/target" "$repo/scripts/test.sh" --no-fail-fast \
    > "$results/fxdax-tests.log" 2>&1 || status=$?
echo "fxdax tests: exit $status" >&2

echo "== guest tests" >&2
e2e_status=0
KRUNFW_LIB_DIR="$libkrunfw_lib" FXDAX_E2E_RESULTS="$results/e2e" "$repo/scripts/test-e2e.sh" "$@" \
    > "$results/e2e.log" 2>&1 || e2e_status=$?
echo "guest tests: exit $e2e_status" >&2
if (( status == 0 )); then status=$e2e_status; fi
exit "$status"
