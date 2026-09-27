#!/usr/bin/env bash
# Run scripts/test.sh on Linux inside a Lima VM. Lima mounts the host home
# read-only, so the repo is copied into the VM first.
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
args=$(printf ' %q' "$@")

limactl shell "$instance" -- bash -lc "
    set -euo pipefail
    rsync -a --delete --exclude target --exclude .git '$repo/' ~/fxdax-test/
    ~/fxdax-test/scripts/test.sh$args
"
