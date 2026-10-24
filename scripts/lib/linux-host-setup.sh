# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# What the scripts that run fxdax on a disposable x86-64 Linux KVM host, as
# root (scripts/lib/*-linux-host.sh), share, sourced by them: provisioning the
# host (host_setup), the tree each builds in (host_tree), the tools it needs,
# at the versions pinned here (host_need), and the record of the host it ran on
# (host_record). Sourcing it defines them and does nothing else. The sourcing
# script sets repo, the source, and results, the directory it writes to. The
# Lima VM's tree is a host_tree too, in the VM's home (scripts/lib/lima.sh).

work=/var/lib/fxdax-test
# The tools host_need installed, which host_record records.
host_needed=()

# host_setup provisions the host, once per host for what lasts: the packages,
# the Rust toolchain and its musl target; and exports the environment the
# builds and tests run in. It makes results absolute.
host_setup() {
    mkdir -p "$work/tools" "$results"
    results=$(cd "$results" && pwd)
    test "$(uname -m)" = x86_64
    test -c /dev/kvm

    export DEBIAN_FRONTEND=noninteractive
    # clang's libraries build Firecracker's bindings, libseccomp its seccompiler,
    # podman the root filesystems of guests that need one, and busybox-static the
    # image of fxfs-guest-docker-dax's containers; flex, bison, bc, cpio and
    # libelf's and OpenSSL's headers build the guest kernel (test-linux-host.sh's
    # FXDAX_GUEST_KERNEL_BUILD). The marker names the list, so a host that
    # installed a different one installs again.
    local packages packages_ready
    packages=(build-essential clang libclang-dev llvm pkg-config musl-tools python3
        curl ca-certificates e2fsprogs podman rsync libseccomp-dev busybox-static
        git flex bison bc cpio libelf-dev libssl-dev sqlite3)
    packages_ready="$work/tools/packages-$(printf '%s\n' "${packages[@]}" | sha256sum | cut -c1-16)"
    if [[ ! -f "$packages_ready" ]]; then
        apt-get -o DPkg::Lock::Timeout=600 update -qq
        apt-get -o DPkg::Lock::Timeout=600 install -y -qq "${packages[@]}"
        touch "$packages_ready"
    fi

    RUSTUP_TOOLCHAIN=$(host_pin rust)
    export CARGO_HOME="$work/tools/cargo" RUSTUP_HOME="$work/tools/rustup" RUSTUP_TOOLCHAIN
    export PATH="$CARGO_HOME/bin:$PATH"
    if [[ ! -x "$CARGO_HOME/bin/cargo" ]]; then
        curl -fsSL https://sh.rustup.rs -o "$work/tools/rustup-init.sh"
        sh "$work/tools/rustup-init.sh" -y --profile minimal --default-toolchain "$RUSTUP_TOOLCHAIN" \
            --no-modify-path
    fi
    rustup target add x86_64-unknown-linux-musl

    # The guest tests' runner isolates each test with `buildah unshare` where
    # buildah is installed, as podman installs it, and buildah's re-execution fails
    # on these hosts (memfd_create: invalid argument), so the tests run without it.
    export FXDAX_NO_UNSHARE=1
}

# host_pin TOOL prints the version of TOOL the hosts run: Rust's, or that of a
# tool cargo installs (host_need).
host_pin() {
    case $1 in
        rust) echo 1.97.1 ;;
        cargo-nextest) echo 0.9.143 ;;
        cargo-mutants) echo 27.1.0 ;;
        *) echo "host_pin: $1 has no version pinned" >&2; return 1 ;;
    esac
}

# host_version TOOL prints the version of TOOL installed, as the tool says it.
host_version() {
    case $1 in
        perf) perf --version ;;
        # Its first line, as cargo-nextest says more.
        cargo-*) cargo "${1#cargo-}" --version | sed -n 1p ;;
        *) echo "host_version: $1 is no tool host_need installs" >&2; return 1 ;;
    esac
}

# host_need TOOL... installs each TOOL, unless it is there: perf, which follows
# the running kernel and so has no pin, or a cargo subcommand, at its pinned
# version (host_pin). host_record records them.
host_need() {
    local tool pin
    for tool in "$@"; do
        case $tool in
            perf)
                if ! command -v perf > /dev/null; then
                    apt-get -o DPkg::Lock::Timeout=600 install -y -qq linux-tools-common \
                        "linux-tools-$(uname -r)" > /dev/null
                fi
                ;;
            *)
                pin=$(host_pin "$tool")
                if [[ "$(host_version "$tool" 2> /dev/null)" != "$tool $pin" ]]; then
                    cargo install "$tool" --version "$pin" --locked
                fi
                ;;
        esac
        host_needed+=("$tool")
    done
}

# host_tree NAME copies the source to the tree NAME, which persists between
# runs so that a run builds only what changed, and points repo at it. By
# content, not times, which a tree unpacked from another checkout's has: only
# what differs is written, which makes it newer than what was built from it, as
# cargo and make need. That holds only against what the tree itself built, so
# each tree builds in target directories of its own, which the copy keeps, as it
# keeps Firecracker's build: in none that another tree builds in too. Git's
# files, which a checkout has, are not the tree's.
#
# A tree copied by times, as with rsync -a, may hold content newer than its
# times, beside what was built from what it replaced: so a copy by content
# writes every file again unless the tree's last copy was by content too, which
# .by-content in the tree records. A copy into the tree by other means that
# deletes what the source lacks, as the scripts' own before (rsync -a --delete),
# deletes it.
host_tree() {
    local tree="$work/$1" compare=--checksum
    mkdir -p "$tree"
    if [[ ! -e $tree/.by-content ]]; then compare=--ignore-times; fi
    rsync -rlpD "$compare" --delete --exclude target --exclude .git \
        --exclude /vendor/firecracker/build --exclude /.by-content "$repo/" "$tree/"
    touch "$tree/.by-content"
    repo=$tree
}

# host_record prints what the run ran on: the host, its CPU and memory, Rust,
# and the tools host_need installed.
host_record() {
    local tool
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    echo "cpus $(nproc)"
    grep MemTotal /proc/meminfo
    rustc --version
    for tool in ${host_needed[@]+"${host_needed[@]}"}; do
        host_version "$tool"
    done
}
