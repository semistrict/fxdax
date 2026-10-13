#!/usr/bin/env bash
# Build the guest kernel with Fxfs in it (scripts/build-guest-kernel.sh) and run libkrun's guest
# test of it, fxfs-guest, on this host: macOS with HVF, which builds the kernel in Lima, or Linux
# with KVM. The kernel is kept in $FXDAX_E2E_RESULTS (default: a new temporary directory), with
# the test's logs.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)

case "$(uname -s)" in
    Darwin)
        built=$("$root/scripts/lib/lima.sh" build-guest-kernel.sh arm64 | tail -1)
        limactl copy "${FXDAX_LIMA_INSTANCE:-default}:$built" "$results/"
        ;;
    Linux)
        built=$("$root/scripts/build-guest-kernel.sh" | tail -1)
        cp "$built" "$results/"
        ;;
    *) echo "Unsupported host: $(uname -s)" >&2; exit 2 ;;
esac

FXDAX_GUEST_KERNEL="$results/$(basename "$built")" FXDAX_E2E_RESULTS="$results" \
    "$root/scripts/test-e2e.sh" fxfs-guest
