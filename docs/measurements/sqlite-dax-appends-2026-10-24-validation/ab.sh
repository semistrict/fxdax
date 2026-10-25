#!/usr/bin/env bash
set -euo pipefail
ab=/home/ramon/sqlite-append-ab
mkdir -p "$ab"
trap 'status=$?; echo "$status" > "$ab/status"' EXIT
cp /var/lib/fxdax-test/guest-kernel-release/out/x86_64/vmlinux "$ab/candidate-vmlinux"
sha256sum "$ab/candidate-vmlinux" > "$ab/candidate-kernel.txt"
mkdir -p "$ab/baseline-source"
tar -xzf /home/ramon/sqlite-baseline-source.tar.gz -C "$ab/baseline-source"
repo=/var/lib/fxdax-test/test-tree
results="$ab/setup"
source "$repo/scripts/lib/linux-host-setup.sh"
host_setup
host_record > "$ab/host.txt"
echo 'Building baseline kernel from a6ee070d9.'
env -u RUSTUP_TOOLCHAIN FXDAX_GUEST_KERNEL_WORK=/var/lib/fxdax-test/guest-kernel-release \
    "$ab/baseline-source/scripts/build-guest-kernel.sh" x86_64 > "$ab/baseline-kernel.log" 2>&1
cp /var/lib/fxdax-test/guest-kernel-release/out/x86_64/vmlinux "$ab/baseline-vmlinux"
sha256sum "$ab/baseline-vmlinux" > "$ab/baseline-kernel.txt"
status=0
for variant in baseline candidate; do
    echo "Measuring $variant: five SQLite iterations."
    mkdir -p "$ab/$variant"
    case_status=0
    FXDAX_GUEST_KERNEL="$ab/$variant-vmlinux" FXDAX_BENCH_CASES=bench-fs-fxfs-guest-dax \
        FXDAX_BENCH_GROUPS=sqlite FXDAX_BENCH_ITERATIONS=5 FXDAX_BENCH_RESULTS="$ab/$variant" \
        "$repo/scripts/bench.sh" 5 > "$ab/$variant/comparison.md" 2> "$ab/$variant/bench.log" || case_status=$?
    echo "$case_status" > "$ab/$variant/status"
    if (( case_status != 0 )); then status=$case_status; fi
    if [[ -d "$ab/$variant/logs" ]]; then
        find "$ab/$variant/logs" -type d \( -name rootfs -o -name extents -o -name '*.img' \) -prune -exec rm -rf {} +
        find "$ab/$variant/logs" -type f \( -name '*.img' -o -name rootfs.ext4 \) -delete
    fi
    echo "$variant: exit $case_status"
done
exit "$status"
