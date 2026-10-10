#!/usr/bin/env bash
# Run libkrun's fxdax guest tests on this host: macOS with HVF, or Linux with
# KVM. Arguments name the test cases (default: virtiofs-fxdax,
# virtiofs-fxdax-nodax, virtiofs-fxdax-volumes and virtiofs-fxdax-memory). Each test's logs are kept
# under $FXDAX_E2E_RESULTS (default: a new temporary directory).
#
# macOS needs Homebrew's llvm and libkrunfw. On Linux, LIBCLANG_PATH names
# libclang's directory where llvm-config cannot, KRUNFW_LIB_DIR the directory
# holding libkrunfw.so.5 where the loader cannot find it, and DAX needs access
# to /dev/userfaultfd (see the README).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
libkrun="$root/vendor/libkrun"
tests=("$@")
if (( ${#tests[@]} == 0 )); then
    tests=(virtiofs-fxdax virtiofs-fxdax-nodax virtiofs-fxdax-volumes virtiofs-fxdax-memory)
fi
results=${FXDAX_E2E_RESULTS:-$(mktemp -d "${TMPDIR:-/tmp}/fxdax-e2e.XXXXXX")}
mkdir -p "$results"
results=$(cd "$results" && pwd)
features=(FXDAX=1 BLK=1 NET=1)
extra=()

case "$(uname -s)" in
    Darwin)
        export LIBCLANG_PATH=${LIBCLANG_PATH:-$(brew --prefix llvm)/lib}
        # make runs with System Integrity Protection stripping DYLD_*, which
        # bindgen needs to load libclang, so libkrun is built here first.
        (cd "$libkrun" && DYLD_FALLBACK_LIBRARY_PATH=$LIBCLANG_PATH \
            cargo build --release --features blk,net,fxdax)
        ;;
    Linux)
        if [[ -z ${LIBCLANG_PATH:-} ]] && command -v llvm-config > /dev/null; then
            LIBCLANG_PATH=$(llvm-config --libdir)
        fi
        if [[ -n ${LIBCLANG_PATH:-} ]]; then export LIBCLANG_PATH; fi
        if [[ -n ${KRUNFW_LIB_DIR:-} ]]; then extra+=(EXTRA_LIBPATH_Linux="$KRUNFW_LIB_DIR"); fi
        ;;
    *) echo "Unsupported host: $(uname -s)" >&2; exit 2 ;;
esac

status=0
for test in "${tests[@]}"; do
    make -C "$libkrun" test "${features[@]}" ${extra[@]+"${extra[@]}"} TEST="$test" \
        TEST_FLAGS="--base-dir $results/$test --keep-all" || status=1
done
echo "Logs: $results" >&2
exit "$status"
