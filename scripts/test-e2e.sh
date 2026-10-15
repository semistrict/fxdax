#!/usr/bin/env bash
# Run libkrun's fxdax guest tests on this host: macOS with HVF, or Linux with
# KVM. Arguments name the test cases (default: virtiofs-fxdax,
# virtiofs-fxdax-nodax, virtiofs-fxdax-volumes and virtiofs-fxdax-memory). Each test's logs are kept
# under $FXDAX_E2E_RESULTS (default: a new temporary directory).
#
# macOS needs Homebrew's llvm and libkrunfw. On Linux, LIBCLANG_PATH names
# libclang's directory where llvm-config cannot, KRUNFW_LIB_DIR the directory
# holding libkrunfw.so.5 where the loader cannot find it, and DAX needs access
# to /dev/userfaultfd (see the README). With FXDAX_E2E_EXTENTD=1 (Linux), the
# guests' extent stores are served by fxdax's extent daemon, which this starts,
# as FXDAX_EXTENTD tells libkrun's extent device, rather than in each VM.
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

if [[ ${FXDAX_E2E_EXTENTD:-} == 1 ]]; then
    cargo build --release --manifest-path "$root/Cargo.toml" -p fxdax --bin fxdax-extentd
    socket="$results/extentd.sock"
    if [[ -e $socket ]]; then
        echo "$socket exists: a daemon may be running" >&2
        exit 2
    fi
    "$root/target/release/fxdax-extentd" "$socket" > "$results/extentd.log" 2>&1 &
    daemon=$!
    trap 'kill "$daemon" 2> /dev/null; rm -f "$socket"' EXIT
    while [[ ! -S $socket ]]; do
        if ! kill -0 "$daemon" 2> /dev/null; then
            echo "the extent daemon exited: see $results/extentd.log" >&2
            exit 1
        fi
        sleep 0.1
    done
    export FXDAX_EXTENTD="$socket"
fi

status=0
for test in "${tests[@]}"; do
    make -C "$libkrun" test "${features[@]}" ${extra[@]+"${extra[@]}"} TEST="$test" \
        TEST_FLAGS="--base-dir $results/$test --keep-all" || status=1
done
echo "Logs: $results" >&2
exit "$status"
