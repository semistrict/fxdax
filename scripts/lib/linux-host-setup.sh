# shellcheck shell=bash
# Provisions a disposable x86-64 Linux KVM host, as root, for the scripts that
# run fxdax there (scripts/lib/test-linux-host.sh, bench-linux-host.sh), which
# source it: the packages, the Rust toolchain, and libkrun's guest kernel in
# $libkrunfw_lib. Each is installed once per host. The sourcing script sets
# results, the directory it writes to.

work=/var/lib/fxdax-test
mkdir -p "$work/tools" "$results"
results=$(cd "$results" && pwd)
test "$(uname -m)" = x86_64
test -c /dev/kvm

export DEBIAN_FRONTEND=noninteractive
# clang's libraries build libkrun's bindings, and podman the root filesystems
# of guests that need one. The marker names the list, so a host that installed
# a different one installs again.
packages=(build-essential clang libclang-dev llvm pkg-config musl-tools python3
    curl ca-certificates e2fsprogs podman rsync)
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

# libkrun's guest kernel, as libkrunfw releases it built.
libkrunfw="$work/libkrunfw-5.6.2"
libkrunfw_lib="$libkrunfw/lib/x86_64-linux-gnu"
if [[ ! -e "$libkrunfw_lib/libkrunfw.so.5" ]]; then
    archive="$work/tools/libkrunfw-5.6.2-x86_64.tgz"
    curl -fsSL https://github.com/libkrun/libkrunfw/releases/download/v5.6.2/libkrunfw-x86_64.tgz \
        -o "$archive"
    echo "016c32ddb2a28aa300382cab33352d63a41a901eb36ee8d1ca24b389791c8b91  $archive" | sha256sum -c -
    mkdir -p "$libkrunfw"
    tar -C "$libkrunfw" -xzf "$archive"
fi

# libkrun's test runner isolates each test with `buildah unshare` where buildah
# is installed, as podman installs it, and buildah's re-execution fails on these
# hosts (memfd_create: invalid argument), so the tests run without it.
export KRUN_NO_UNSHARE=1
