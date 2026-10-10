#!/usr/bin/env bash
# Run one of the repo's scripts on Linux inside a Lima VM: lima.sh SCRIPT [ARG...]
# runs scripts/SCRIPT with the arguments there. Lima mounts the host home
# read-only, so the repo is copied into the VM first; what the script leaves
# behind stays in the VM.
#
# LIBCLANG_PATH and KRUNFW_LIB_DIR, which libkrun's guest tests need, default
# to the VM's ~/libclang and ~/libkrunfw/lib/<arch>-linux-gnu, where they exist.
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
script=$(printf '%q' "${1:?script required}")
shift
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
    LIBCLANG_PATH=\$libclang KRUNFW_LIB_DIR=\$krunfw ~/fxdax-test/scripts/$script$args
"
