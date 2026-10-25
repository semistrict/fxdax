#!/usr/bin/env bash
set -euo pipefail
ab=/home/ramon/sqlite-append-ab
trap 'status=$?; echo "$status" > "$ab/repeat-status"' EXIT
repo=/var/lib/fxdax-test/test-tree
results="$ab/setup"
source "$repo/scripts/lib/linux-host-setup.sh"
host_setup
status=0
for variant in candidate baseline; do
    name="$variant-repeat"
    echo "Measuring $name: five SQLite iterations, reversed order."
    mkdir -p "$ab/$name"
    case_status=0
    FXDAX_GUEST_KERNEL="$ab/$variant-vmlinux" FXDAX_BENCH_CASES=bench-fs-fxfs-guest-dax \
        FXDAX_BENCH_GROUPS=sqlite FXDAX_BENCH_ITERATIONS=5 FXDAX_BENCH_RESULTS="$ab/$name" \
        "$repo/scripts/bench.sh" 5 > "$ab/$name/comparison.md" 2> "$ab/$name/bench.log" || case_status=$?
    echo "$case_status" > "$ab/$name/status"
    if (( case_status != 0 )); then status=$case_status; fi
    find "$ab/$name/logs" -type d \( -name rootfs -o -name extents -o -name '*.img' \) -prune -exec rm -rf {} +
    find "$ab/$name/logs" -type f \( -name '*.img' -o -name rootfs.ext4 \) -delete
    echo "$name: exit $case_status"
done
exit "$status"
