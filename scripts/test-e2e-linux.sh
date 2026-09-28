#!/usr/bin/env bash
# Run scripts/test-e2e.sh on Linux inside a Lima VM. Lima mounts the host home
# read-only, so the repo is copied into the VM first; the logs stay in the VM,
# under the directory the script prints.
#
# LIBCLANG_PATH and KRUNFW_LIB_DIR default to the VM's ~/libclang and
# ~/libkrunfw/lib/<arch>-linux-gnu, where they exist.
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
args=
if (( $# > 0 )); then args=$(printf ' %q' "$@"); fi
libclang=$(printf '%q' "${LIBCLANG_PATH:-}")
krunfw=$(printf '%q' "${KRUNFW_LIB_DIR:-}")

limactl shell "$instance" -- bash -lc "
    set -euo pipefail
    rsync -a --delete --exclude target --exclude .git '$repo/' ~/fxdax-test/
    libclang=$libclang krunfw=$krunfw
    if [[ -z \$libclang && -d ~/libclang ]]; then libclang=~/libclang; fi
    if [[ -z \$krunfw && -d ~/libkrunfw/lib/\$(uname -m)-linux-gnu ]]; then
        krunfw=~/libkrunfw/lib/\$(uname -m)-linux-gnu
    fi
    LIBCLANG_PATH=\$libclang KRUNFW_LIB_DIR=\$krunfw ~/fxdax-test/scripts/test-e2e.sh$args
"
