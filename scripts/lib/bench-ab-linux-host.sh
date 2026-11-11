#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# An A/B of the fs bench (scripts/bench.sh) on an x86-64 Linux KVM host, as root
# (scripts/bench-ab-host.sh drives it, TASK-276): each side's tree built once, in a tree of its own
# (host_tree), into artifacts of its own, the guest kernel among them (test-e2e.sh's
# FXDAX_E2E_BUILD_ONLY); then ROUNDS rounds, each of both sides, A B, then B A, and so on, each a
# run of the bench against its side's artifacts, building nothing (FXDAX_E2E_PREBUILT), so that
# what drifts on the host moves both alike, and a round takes the workloads' time alone. Each
# round's results go to RESULTS/NAME-rN, and scripts/bench-ab.py's comparison of the sides to
# RESULTS/ab.md, the control FXDAX_AB_CONTROL's case (default ext4-guest-kernel; empty: none).
#
# With FXDAX_AB_SUITE=qualify (default fs, the bench), each round is the qualification instead
# (scripts/qualify.sh, TASK-330.4), of LangSmith's pin against a candidate: its harness, the
# runner and the guest agent, this tree's for both sides (test-e2e.sh's FXDAX_E2E_HARNESS), as a
# pin from before the qualification's cases lacks them; what each side builds, its extent daemon,
# its Firecracker and its guest kernel, what it tests. No control by default: what moves, and
# each side's failed checks, ab.md says (scripts/bench-ab.py).
#
# Usage: bench-ab-linux-host.sh TOOLS RESULTS A_SOURCE B_SOURCE ROUNDS A_NAME B_NAME
#
# FXDAX_AB_PHASE is what of it runs: `build` builds the sides (and the qualification's harness)
# into the work directory, and stops; `rounds` runs the rounds of what a build there made, and
# compares them; unset, both. scripts/bench-ab-host.sh runs the build as work and the rounds alone
# as a measurement (fxjob --bench), so that compiling takes no measurement's cores.
#
# TOOLS is the tree whose scripts run, this one's: the sides' trees may be of before them (an
# A/B's base), and build and boot their own code (test-e2e.sh's FXDAX_E2E_ROOT). B_SOURCE `same`
# is A's tree, built once, for an A/B of one build that FXDAX_AB_A_ENV and FXDAX_AB_B_ENV tell
# apart: NAME=VALUE words each side's rounds run with, such as
# FXDAX_GUEST_CMDLINE=virtio_extent.affine=0. FXDAX_BENCH_CASES, FXDAX_BENCH_GROUPS,
# FXDAX_BENCH_ITERATIONS (each round's, default 2) and the like are the same for both sides.
# FXDAX_GUEST_KERNEL_BUILD is the guest kernels' build (default release). FXDAX_AB_BUILD_NICE is
# the niceness the sides build at (default 0), the rounds' unchanged: on a host others share, with
# CARGO_BUILD_JOBS and FXDAX_GUEST_KERNEL_JOBS (scripts/build-guest-kernel.sh) to bound them.
set -euo pipefail

# Prints the sides' order of each of ROUNDS rounds, a line each: `a b`, then `b a`, and so on, so
# that neither side always runs first, after the other's warming of the host.
ab_order() {
    local r
    for (( r = 1; r <= $1; r++ )); do
        if (( r % 2 )); then echo "a b"; else echo "b a"; fi
    done
}

# Sourced, as by the scripts' tests, it defines its functions and does nothing else.
[[ ${BASH_SOURCE[0]} == "$0" ]] || return 0

phase=${FXDAX_AB_PHASE:-all}
[[ $phase == all || $phase == build || $phase == rounds ]] ||
    { echo "FXDAX_AB_PHASE is build or rounds, or unset for both" >&2; exit 2; }
tools=$(cd "${1:?tools required}" && pwd)
results=${2:?results directory required}
a_source=${3:?A source required} b_source=${4:?B source required}
rounds=${5:?rounds required} a_name=${6:?A name required} b_name=${7:?B name required}
[[ $rounds =~ ^[1-9][0-9]*$ ]] || { echo "ROUNDS is a count" >&2; exit 2; }
repo=$tools
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
kernel_build=${FXDAX_GUEST_KERNEL_BUILD:-release}
[[ $kernel_build == release || $kernel_build == debug ]] ||
    { echo "FXDAX_GUEST_KERNEL_BUILD is release or debug" >&2; exit 2; }
unset FXDAX_GUEST_KERNEL_BUILD
iterations=${FXDAX_BENCH_ITERATIONS:-2}
suite=${FXDAX_AB_SUITE:-fs}
[[ $suite == fs || $suite == qualify ]] || { echo "FXDAX_AB_SUITE is fs or qualify" >&2; exit 2; }
read -ra a_env <<< "${FXDAX_AB_A_ENV:-}"
read -ra b_env <<< "${FXDAX_AB_B_ENV:-}"
# The sides' builds' niceness, their guest kernels' too (host_kernel).
host_build_nice=${FXDAX_AB_BUILD_NICE:-0}

# Each side's tree, by side, which build makes.
declare -A trees

# Builds side SIDE from SOURCE once: its tree, trees[SIDE], its guest kernel and its artifacts, in
# $work/ab-artifacts-SIDE.
build() {
    local side=$1 source=$2 tree
    repo=$source
    host_tree "bench-ab-$side"
    tree=$repo
    echo "Building side $side from $source" >&2
    host_kernel_log=$results/guest-kernel-$side.log \
        host_kernel "$kernel_build" "$work/guest-kernel-ab-$side-$kernel_build"
    rm -rf "$work/ab-artifacts-$side"
    nice -n "$host_build_nice" env FXDAX_E2E_ROOT="$tree" \
        FXDAX_E2E_BUILD_ONLY="$work/ab-artifacts-$side" FXDAX_E2E_RESULTS="$results/build-$side" \
        "$tools/scripts/test-e2e.sh" > "$results/build-$side.log" 2>&1
    cp "$host_kernel_image" "$work/ab-artifacts-$side/vmlinux"
    sha256sum "$work/ab-artifacts-$side/vmlinux" >> "$work/ab-artifacts-$side/artifacts.txt"
    cp "$work/ab-artifacts-$side/artifacts.txt" "$results/artifacts-$side.txt"
    trees[$side]=$tree
}

# What a build made, which the rounds run: each side's tree, and the harness.
built=$work/ab-built
harness=

# Builds both sides (build), and the qualification's harness, this tree's, the same for both
# sides, which a side of this tree's own revision built already; and records what it made, in
# $built and the results' host.txt.
build_sides() {
    local side
    build a "$a_source"
    if [[ $b_source == same ]]; then
        trees[b]=${trees[a]}
        mkdir -p "$work/ab-artifacts-b"
        cp -a "$work/ab-artifacts-a/." "$work/ab-artifacts-b/"
        cp "$results/artifacts-a.txt" "$results/artifacts-b.txt"
    else
        build b "$b_source"
    fi

    if [[ $suite == qualify ]]; then
        for side in a b; do
            if cmp -s "$tools/source-revision.txt" "${trees[$side]}/source-revision.txt"; then
                harness=$work/ab-artifacts-$side
                break
            fi
        done
        if [[ -z $harness ]]; then
            harness=$work/ab-harness
            echo "Building the harness from $tools" >&2
            nice -n "$host_build_nice" env FXDAX_E2E_BUILD_ONLY="$harness" \
                FXDAX_E2E_RESULTS="$results/build-harness" "$tools/scripts/test-e2e.sh" \
                > "$results/build-harness.log" 2>&1
        fi
        cp "$harness/artifacts.txt" "$results/artifacts-harness.txt"
    fi

    {
        host_record
        echo "Suite $suite${harness:+, the harness of $(sed -n 's/^source //p' "$harness/artifacts.txt")}"
        echo "A $a_name: $a_source ${a_env[*]:-}"
        echo "B $b_name: $b_source ${b_env[*]:-}"
    } > "$results/host.txt"
    printf 'a %s\nb %s\nharness %s\n' "${trees[a]}" "${trees[b]}" "$harness" > "$built"
}

# Reads what a build made from $built.
load_build() {
    local key value
    [[ -f $built ]] || { echo "$work holds no build: FXDAX_AB_PHASE=build first" >&2; exit 2; }
    while read -r key value; do
        case $key in
            a | b) trees[$key]=$value ;;
            harness) harness=$value ;;
        esac
    done < "$built"
}

if [[ $phase == rounds ]]; then
    load_build
else
    build_sides
fi
if [[ $phase == build ]]; then
    echo "Built in ${SECONDS}s." | tee -a "$results/host.txt"
    exit 0
fi

# Runs side SIDE's round ROUND, with its name and its environment's words.
round() {
    local side=$1 round=$2 name
    local -a env
    if [[ $side == a ]]; then name=$a_name env=(${a_env[@]+"${a_env[@]}"}); else
        name=$b_name env=(${b_env[@]+"${b_env[@]}"}); fi
    set -- ${env[@]+"${env[@]}"}
    echo "Round $round: $name" >&2
    if [[ $suite == qualify ]]; then
        env "$@" FXDAX_E2E_PREBUILT="$work/ab-artifacts-$side" FXDAX_E2E_HARNESS="$harness" \
            FXDAX_GUEST_KERNEL="$work/ab-artifacts-$side/vmlinux" \
            "$tools/scripts/qualify.sh" "$results/$name-r$round" > "$results/$name-r$round.md" \
            2> "$results/$name-r$round.log"
        return
    fi
    env "$@" FXDAX_E2E_ROOT="${trees[$side]}" FXDAX_E2E_PREBUILT="$work/ab-artifacts-$side" \
        FXDAX_GUEST_KERNEL="$work/ab-artifacts-$side/vmlinux" \
        FXDAX_BENCH_RESULTS="$results/$name-r$round" \
        "$tools/scripts/bench.sh" "$iterations" > "$results/$name-r$round.md" \
        2> "$results/$name-r$round.log"
}

status=0 r=0
while read -r first second; do
    r=$((r + 1))
    round "$first" "$r" || status=1
    round "$second" "$r" || status=1
done < <(ab_order "$rounds")

control=${FXDAX_AB_CONTROL-ext4-guest-kernel}
if [[ $suite == qualify ]]; then control=${FXDAX_AB_CONTROL-}; fi
sides=()
for (( r = 1; r <= rounds; r++ )); do sides+=("$results/$a_name-r$r"); done
sides+=(-- "$b_name")
for (( r = 1; r <= rounds; r++ )); do sides+=("$results/$b_name-r$r"); done
"$tools/scripts/bench-ab.py" ${control:+--control "$control"} "$a_name" "${sides[@]}" \
    > "$results/ab.md" || status=1
# The guests' root filesystems and the stores, which a failed case leaves, are not results.
find "$results" -type d \( -name rootfs -o -name extents -o -name '*.img' \) -prune \
    -exec rm -rf {} +
find "$results" -type f \( -name '*.img' -o -name rootfs.ext4 \) -delete
echo "Took ${SECONDS}s." | tee -a "$results/host.txt"
cat "$results/ab.md"
exit "$status"
