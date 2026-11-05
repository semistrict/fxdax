#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run clippy's rules (clippy.toml), the test suite, extent_remote's simulation, the concurrency
# tests under Shuttle, the scripts' tests (scripts/tests) and the mutant runners' checks, and on
# Linux the guest tests' own tests (guest/tests, both halves): what CI runs, a part a job.
# Arguments are passed to the suite's and the simulation's `cargo nextest run`. FXDAX_TEST_PARTS
# names the parts to run, separated by spaces (default: all of clippy, workspace, sim, shuttle,
# scripts, and guest on Linux).
#
# Unicode tests compare against the Unicode Character Database, which is
# downloaded and checksummed on first use.
set -euo pipefail

VERSION=12.1.0
SHA256=25ba51a0d4c6fa41047b7a5e5733068d4a734588f055f61e85f450097834a0a6
root="$(cd "$(dirname "$0")/.." && pwd)"
zip="$root/target/UCD-$VERSION.zip"

if [ ! -f "$zip" ]; then
    mkdir -p "$root/target"
    curl -fsSL -o "$zip" "https://www.unicode.org/Public/$VERSION/ucd/UCD.zip"
fi
echo "$SHA256  $zip" | shasum -a 256 -c - >/dev/null

# glibc's fatal errors (a heap found corrupted, a stack smashed) on standard error, which the tests
# keep, rather than the terminal they have none of: an abort said nothing else (TASK-100).
export LIBC_FATAL_STDERR_=1

parts="clippy workspace sim shuttle scripts"
[[ $(uname) != Linux ]] || parts+=" guest"
parts=${FXDAX_TEST_PARTS:-$parts}
part() { # name: whether FXDAX_TEST_PARTS has it
    [[ " $parts " == *" $1 "* ]]
}

# Clippy's rules the code keeps (clippy.toml): no blocking call in async code unless allowed, with
# why (TASK-44.20). First, as it fails at once.
if part clippy; then
    UCD_ZIP="$zip" cargo clippy --manifest-path "$root/Cargo.toml" --workspace --tests -- \
        -A clippy::all -D clippy::disallowed_methods
fi
# extent_remote's simulation: its profiles' soaks, and its rules' seed searches, each until a seed
# with the rule off finds what the rule prevents, up to 20,000. Its own part, built with the
# profile sim, optimized with a debug build's checks: unoptimized, in the suite's build, four
# searches took 20 to 27 minutes each, which ran CI's workspace job out of time (TASK-188).
sim='package(extent_remote) & test(/^sim::/)'
if part workspace; then
    UCD_ZIP="$zip" cargo nextest run --manifest-path "$root/Cargo.toml" --workspace \
        -E "not ($sim)" "$@"
fi
if part sim; then
    cargo nextest run --manifest-path "$root/Cargo.toml" --cargo-profile sim -p extent_remote \
        -E "$sim" "$@"
fi
# The concurrency tests under Shuttle, built with each crate's `shuttle` feature, which only they
# can run (TASK-44).
if part shuttle; then
    cargo nextest run --manifest-path "$root/Cargo.toml" -p extent_store --features shuttle \
        --test shuttle
    cargo nextest run --manifest-path "$root/Cargo.toml" -p extent_store --features shuttle --lib \
        shuttle_tests::
    cargo nextest run --manifest-path "$root/Cargo.toml" -p physmem --features shuttle --lib \
        shuttle::
    cargo nextest run --manifest-path "$root/Cargo.toml" -p fxdax --features shuttle --lib \
        shuttle_tests::
    # The followers' concurrency protocol, under Shuttle (TASK-42.5).
    cargo nextest run --manifest-path "$root/Cargo.toml" -p follower --features shuttle
    # The followers' simulation: Fxfs as writer and followers on an extent store (TASK-42.1).
    cargo nextest run --manifest-path "$root/Cargo.toml" -p follower_sim --features shuttle
fi
# The scripts' own tests, and that each hand mutant's patch or edits apply as they are: one that
# stopped applying would test nothing (guest/sim's need a kernel tree, so are CI's guest-sim job's,
# but for those of its model of Fxfs, which crates/fxfs-sim-diff runs).
if part scripts; then
    python3 -m unittest discover -s "$root/scripts/tests"
    "$root/crates/fxfs-posix/mutants.sh" --check
    "$root/crates/fxfs-sim-diff/mutants.sh" --check
    "$root/scripts/guest-mutants-gce.py" --check
fi
# The guest tests' own tests, the host's half and the guest's, which build on Linux alone: their
# harness's (guest/tests/test_cases/src/vm.rs, script_vm.rs, the runner's) and the guest agent's,
# some of which mount loop devices, as root, with mkfs.ext4 and mkfs.xfs: CI runs this part as
# root.
if part guest; then
    cargo nextest run --manifest-path "$root/guest/tests/Cargo.toml" -p test_cases --features host
    cargo nextest run --manifest-path "$root/guest/tests/Cargo.toml" -p test_cases --features guest
    cargo nextest run --manifest-path "$root/guest/tests/Cargo.toml" -p runner
fi
