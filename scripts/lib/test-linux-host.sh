#!/usr/bin/env bash
# Qualify fxdax on a disposable x86-64 Linux KVM host, as root. It runs the
# workspace's tests, then libkrun's guest test $3 (default virtiofs-fxdax),
# whose DAX window takes KVM's faults through userfaultfd.
#
# Usage: test-linux-host.sh SOURCE RESULTS [TEST]
set -euo pipefail
repo=$(cd "${1:?source required}" && pwd)
results=${2:?results directory required}
e2e_test=${3:-virtiofs-fxdax}
work=/var/lib/fxdax-test
mkdir -p "$work/tools" "$results"
results=$(cd "$results" && pwd)
test "$(uname -m)" = x86_64
test -c /dev/kvm

export DEBIAN_FRONTEND=noninteractive
# bc through libelf-dev build libkrunfw's guest kernel, and clang's libraries
# libkrun's bindings. The marker names the list, so a host that installed a
# shorter one installs again.
packages=(build-essential clang libclang-dev llvm pkg-config musl-tools python3
    python3-pyelftools curl ca-certificates patch xz-utils bc bison flex elfutils
    libelf-dev e2fsprogs)
packages_ready="$work/tools/packages-$(printf '%s\n' "${packages[@]}" | sha256sum | cut -c1-16)"
if [[ ! -f "$packages_ready" ]]; then
    apt-get -o DPkg::Lock::Timeout=600 update -qq
    apt-get -o DPkg::Lock::Timeout=600 install -y -qq "${packages[@]}"
    touch "$packages_ready"
fi

export CARGO_HOME="$work/tools/cargo" RUSTUP_HOME="$work/tools/rustup"
export RUSTUP_TOOLCHAIN=1.97.1
export PATH="$CARGO_HOME/bin:$PATH"
if [[ ! -x "$CARGO_HOME/bin/cargo" ]]; then
    curl -fsSL https://sh.rustup.rs -o "$work/tools/rustup-init.sh"
    sh "$work/tools/rustup-init.sh" -y --profile minimal --default-toolchain "$RUSTUP_TOOLCHAIN" \
        --no-modify-path
fi
rustup target add x86_64-unknown-linux-musl
if [[ "$(cargo nextest --version 2>/dev/null | head -1)" != 'cargo-nextest 0.9.143' ]]; then
    cargo install cargo-nextest --version 0.9.143 --locked
fi

# libkrun's guest kernel.
libkrunfw="$work/libkrunfw"
libkrunfw_lib="$libkrunfw/lib/x86_64-linux-gnu"
if [[ ! -e "$libkrunfw_lib/libkrunfw.so.5" ]]; then
    archive="$work/tools/libkrunfw-5.6.2.tar.gz"
    curl -fsSL https://github.com/containers/libkrunfw/archive/refs/tags/v5.6.2.tar.gz -o "$archive"
    echo "df45d649fcbd07a4d0ca03fa836b2f640fdcf9b40187f3eb7023259c6e83d582  $archive" | sha256sum -c -
    [[ -d "$work/tools/libkrunfw-5.6.2" ]] || tar -C "$work/tools" -xzf "$archive"
    make -C "$work/tools/libkrunfw-5.6.2" -j"$(nproc)"
    make -C "$work/tools/libkrunfw-5.6.2" install PREFIX="$libkrunfw"
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

echo "== libkrun $e2e_test" >&2
e2e_status=0
(
    cd "$repo/vendor/libkrun"
    export LIBCLANG_PATH
    LIBCLANG_PATH=$(llvm-config --libdir)
    make FXDAX=1 BLK=1 NET=1 -j"$(nproc)"
    make test FXDAX=1 BLK=1 NET=1 TEST="$e2e_test" EXTRA_LIBPATH_Linux="$libkrunfw_lib" \
        TEST_FLAGS="--base-dir $results/e2e --keep-all"
) > "$results/e2e.log" 2>&1 || e2e_status=$?
echo "libkrun $e2e_test: exit $e2e_status" >&2
if (( status == 0 )); then status=$e2e_status; fi
exit "$status"
