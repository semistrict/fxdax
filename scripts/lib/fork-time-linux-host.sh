#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# How long a fork of a large base takes, on trees of fxdax's (TASK-165.7): on a disposable x86-64
# Linux host, as root, for each LABEL=TREE, builds TREE's extent daemon and fxdaxctl, starts the
# daemon on a file bucket and a part cache of its own, imports a base of FXDAX_FORK_TIME_FILES
# files (2,560) of FXDAX_FORK_TIME_FILE_MIB MiB (4) of random bytes, 10 GiB, which compress no
# smaller, waits for its uploads, then forks it FXDAX_FORK_TIME_FORKS times (5), each fork timed,
# and the fork's first stat after it, which opens it. Every command goes through the daemon's
# control API, as a host's control plane's would, which both trees have the same way. One image,
# made once, for every tree. RESULTS/LABEL.tsv has `step iteration seconds` lines (import, sync,
# fork, stat), RESULTS/LABEL-stat.txt what each fork's stat said, and RESULTS/LABEL-stats.txt the
# daemon's counters, where its tree keeps them.
#
# Usage: fork-time-linux-host.sh SOURCE RESULTS LABEL=TREE...
set -euo pipefail
# SOURCE, this script's tree, is the host scripts' first argument (gce_host_command); the trees
# timed are the LABEL=TREEs.
: "${1:?source required}"
results=${2:?results directory required}
shift 2
(( $# > 0 )) || { echo "Name a LABEL=TREE to time" >&2; exit 2; }
source "$(dirname "${BASH_SOURCE[0]}")/linux-host-setup.sh"
host_setup
host_record > "$results/host.txt"

files=${FXDAX_FORK_TIME_FILES:-2560}
mib=${FXDAX_FORK_TIME_FILE_MIB:-4}
forks=${FXDAX_FORK_TIME_FORKS:-5}
scratch=$work/fork-time
mkdir -p "$scratch"

# The base's image: a tar archive of one layer, as `docker export` makes, of random files.
layer=$scratch/layer-$files-$mib.tar
if [[ ! -f $layer ]]; then
    echo "== the image: $files files of $mib MiB" >&2
    made=$scratch/layer
    if [[ -e $made ]]; then rm -r -- "$made"; fi
    mkdir -p "$made/data"
    for (( i = 0; i < files; i++ )); do
        head -c $(( mib << 20 )) /dev/urandom > "$made/data/$i"
    done
    tar -cf "$layer.new" -C "$made" data
    mv "$layer.new" "$layer"
    rm -r -- "$made"
fi
# The store's capacity: the image's data, half again for Fxfs's own, and a GiB.
capacity=$(( (files * mib << 20) * 3 / 2 + (1 << 30) ))

for pair in "$@"; do
    label=${pair%%=*}
    tree=$(cd "${pair#*=}" && pwd)
    [[ $label =~ ^[a-z0-9-]+$ ]] || { echo "$label: a label is a word" >&2; exit 2; }
    echo "== $label: building $tree" >&2
    target=$scratch/target-$label
    (cd "$tree" && CARGO_TARGET_DIR=$target cargo build --release -q -p fxdax \
        --bin fxdax-extentd --bin fxdaxctl)
    bin=$target/release
    dir=$scratch/$label
    if [[ -e $dir ]]; then rm -r -- "$dir"; fi
    mkdir -p "$dir/stores" "$dir/bucket" "$dir/cache"
    out=$results/$label.tsv
    : > "$out"
    echo "== $label: timing" >&2
    FXDAX_EXTENTD_REMOTE="file://$dir/bucket/" FXDAX_EXTENTD_TOKEN=1 \
        FXDAX_EXTENTD_CACHE="$dir/cache" FXDAX_EXTENTD_GRPC="$dir/extentd.control" \
        FXDAX_EXTENTD_STATS="$results/$label-stats.txt" \
        "$bin/fxdax-extentd" "$dir/extentd.sock" $(( 8 << 30 )) > "$results/$label-extentd.log" 2>&1 &
    daemon=$!
    for _ in {1..600}; do
        [[ -S $dir/extentd.control ]] && break
        kill -0 "$daemon" || { echo "$label: the daemon exited" >&2; exit 1; }
        sleep 0.1
    done
    export FXDAX_EXTENTD_GRPC=$dir/extentd.control
    # timed STEP ITERATION COMMAND... runs COMMAND and records how long it took.
    timed() {
        local step=$1 iteration=$2 start end
        shift 2
        start=$(date +%s.%N)
        "$@" >> "$results/$label-stat.txt" 2>&1
        end=$(date +%s.%N)
        printf '%s\t%s\t%.3f\n' "$step" "$iteration" "$(echo "$end - $start" | bc)" | tee -a "$out" >&2
    }
    timed import 0 "$bin/fxdaxctl" import "$dir/stores/base" "$capacity" "$layer"
    timed sync 0 "$bin/fxdaxctl" sync "$dir/stores/base"
    timed stat 0 "$bin/fxdaxctl" stat "$dir/stores/base"
    for (( i = 1; i <= forks; i++ )); do
        timed fork "$i" "$bin/fxdaxctl" fork "$dir/stores/base" "$dir/stores/fork-$i"
        timed stat "$i" "$bin/fxdaxctl" stat "$dir/stores/fork-$i"
    done
    kill -TERM "$daemon"
    wait "$daemon" || echo "$label: the daemon exited $?" >&2
    du -sb "$dir/stores" "$dir/bucket" "$dir/cache" > "$results/$label-disk.txt"
    rm -r -- "$dir"
done
