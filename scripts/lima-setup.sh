#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Create, and provision, the Lima VM that scripts/test-linux.sh, test-e2e-linux.sh,
# bench-linux.sh, test-guest-kernel.sh and lib/lima.sh run fxdax's Linux side in on macOS:
# Ubuntu 24.04 on Apple's virtualization with nested virtualization (KVM for the guests), 8
# CPUs, 16 GiB, a 256 GiB disk. FXDAX_LIMA_INSTANCE names it (default: `default`). Running it
# again provisions what is missing, and changes nothing else.
#
# Inside it: the packages Firecracker, the guest kernel (cross-built for x86-64 too) and the
# guests' root filesystems need; Rust and cargo-nextest (the guest's pinned nightly installs
# itself, from guest/rust/rust-toolchain.toml), with the musl targets the guest tests' agent is
# built for; and libseccomp 2.6 in /usr/local, which Firecracker's seccompiler needs and Ubuntu
# 24.04 lacks (it has 2.5.5).
set -euo pipefail

instance="${FXDAX_LIMA_INSTANCE:-default}"

if ! limactl list --quiet | grep -qx "$instance"; then
    limactl create --tty=false --name "$instance" --vm-type vz --cpus 8 --memory 16 --disk 256 \
        --nested-virt template:default
fi
if [[ $(limactl list --format '{{.Status}}' "$instance") != Running ]]; then
    limactl start --tty=false "$instance"
fi

limactl shell "$instance" -- bash -s <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

packages=(build-essential clang libclang-dev llvm pkg-config musl-tools python3 curl
    ca-certificates e2fsprogs podman rsync git unzip xz-utils
    flex bison bc cpio libelf-dev libssl-dev gcc-x86-64-linux-gnu gperf libseccomp-dev elfutils
    coccinelle)
missing=()
for package in "${packages[@]}"; do
    dpkg-query -W -f '${Status}' "$package" 2>/dev/null | grep -q 'install ok installed' ||
        missing+=("$package")
done
if (( ${#missing[@]} > 0 )); then
    sudo apt-get -o DPkg::Lock::Timeout=600 update -qq
    sudo apt-get -o DPkg::Lock::Timeout=600 install -y -qq "${missing[@]}"
fi

# The guests' KVM, and DAX's userfaultfd, through the kvm group (see the README).
sudo usermod -aG kvm "$USER"
echo 'KERNEL=="userfaultfd", GROUP="kvm", MODE="0660"' |
    sudo tee /etc/udev/rules.d/99-userfaultfd.rules >/dev/null
sudo udevadm control --reload && sudo udevadm trigger --name-match=userfaultfd

if [[ ! -x ~/.cargo/bin/cargo ]]; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
source ~/.cargo/env
rustup component add rustfmt clippy
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
command -v cargo-nextest >/dev/null || cargo install --locked cargo-nextest

if [[ ! -e /usr/local/lib/libseccomp.so.2.6.0 ]]; then
    build=$(mktemp -d)
    curl -fsSL https://github.com/seccomp/libseccomp/releases/download/v2.6.0/libseccomp-2.6.0.tar.gz \
        -o "$build/libseccomp.tar.gz"
    echo "83b6085232d1588c379dc9b9cae47bb37407cf262e6e74993c61ba72d2a784dc  $build/libseccomp.tar.gz" |
        sha256sum -c - >/dev/null
    tar -C "$build" -xzf "$build/libseccomp.tar.gz"
    (cd "$build/libseccomp-2.6.0" && ./configure --prefix=/usr/local -q && make -s -j"$(nproc)" &&
        sudo make -s install)
    sudo ldconfig
fi
echo "provisioned: $(rustc --version), $(cargo nextest --version | head -n 1)"
EOF

# Lima's shells reuse one login, which a group added since lacks: a restart starts another.
if ! limactl shell "$instance" -- id -nG | grep -qw kvm; then
    limactl stop "$instance"
    limactl start --tty=false "$instance"
fi
limactl shell "$instance" -- bash -c 'test -r /dev/kvm && test -r /dev/userfaultfd'
