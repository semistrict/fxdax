#!/bin/bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Puts each mutant of guest/mutants that changes guest/sim's model of Fxfs (guest/sim/fxfs/fxfs.c)
# into a copy of the model, runs the model's comparison with Fxfs against it (FXDAX_SIM_MODEL), and
# says its fate (guest/mutants/README.md). One whose header names the model as its kernel
# (`Kernel: model`) is run against its killer alone (`Killer: fxfs-sim-diff TEST`): killed if that
# fails, MISSED if it passes, BROKEN if it doesn't apply or build; this fails unless each is killed.
# Another, the simulator's, is run against every test here, and its fate only said: found or not.
#
#   mutants.sh [--check] [-c CASES] [NAME...]
#
# CASES is how many random sequences each tries, unshrunk. The tree stays as it is. Each patch
# applies as it is, with no fuzz. --check runs nothing: it applies and reverses each patch in a
# copy, in a second, as CI does. A patch that doesn't apply fails either. No grep: what a shell's
# PATH calls grep may be something else.

set -u -o pipefail
cd "$(dirname "$0")/../.."

check=""
if [[ ${1:-} == --check ]]; then
    check=1
    shift
fi
cases=64
while getopts c: opt; do
    case $opt in
        c) cases=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))

model=guest/sim/fxfs/fxfs.c
mutants=guest/mutants

# A field of a mutant's header: `# FIELD: value`.
field() { # name, field
    sed -n "s/^# $2: //p" "$mutants/$1.patch"
}

names=("$@")
if [[ ${#names[@]} == 0 ]]; then
    for patch in "$mutants"/*.patch; do
        [[ -z $(sed -n "\|^+++ b/$model\$|p" "$patch") ]] || names+=("$(basename "$patch" .patch)")
    done
fi

# A copy of the model with mutant `name` in, in a new directory: the copy's path.
patched() { # name
    local copy
    copy=$(mktemp -d)
    mkdir -p "$copy/$(dirname "$model")"
    cp "$model" "$copy/$model"
    patch -s -F0 -p1 -d "$copy" < "$mutants/$1.patch" > /dev/null || { rm -r "$copy"; return 1; }
    echo "$copy/$model"
}

broken=0
if [[ -n $check ]]; then
    for name in "${names[@]}"; do
        if ! copy=$(patched "$name"); then
            echo "$name doesn't apply" >&2
            broken=$((broken + 1))
            continue
        fi
        patch -s -F0 -R -p1 -d "${copy%/"$model"}" < "$mutants/$name.patch" > /dev/null &&
            cmp -s "$model" "$copy" || { echo "$name doesn't reverse" >&2; broken=$((broken + 1)); }
        rm -r "${copy%/"$model"}"
    done
    echo "mutants: ${#names[@]} checked, $broken broken"
    exit $((broken > 0))
fi

# What a failing run said: the model's panic, or the first operation the comparison failed after.
said() { # log
    sed -nE -e '/the model: /{s/.*(the model: [^"\\]*).*/\1/p;q;}' \
        -e '/op [0-9]+/{s/.*((after )?op [0-9]+[^"\\]{0,200}).*/\1/p;q;}' "$1"
}

lived=0
printf '%-8s %-44s %-34s %s\n' fate mutant killer 'what it said'
for name in "${names[@]}"; do
    kernel=$(field "$name" Kernel)
    read -r crate test < <(field "$name" Killer)
    if [[ $kernel == model ]]; then
        killer="$crate $test"
        filter=$test
    else
        killer="(every test; Kernel: $kernel)"
        filter=""
    fi
    if ! copy=$(patched "$name"); then
        printf '%-8s %-44s %-34s %s\n' BROKEN "$name" "$killer" "its patch doesn't apply"
        lived=1
        continue
    fi
    log=$(mktemp)
    began=$SECONDS
    FXDAX_SIM_MODEL=$copy FXDAX_SIM_DIFF_CASES=$cases PROPTEST_MAX_SHRINK_ITERS=0 \
        cargo nextest run -p fxfs-sim-diff ${filter:+"$filter"} > "$log" 2>&1
    status=$?
    took="$((SECONDS - began)) s"
    case $status in
        0) fate=MISSED said="$took: it passed" ;;
        100) fate=killed said="$took: $(said "$log")" ;;
        *) fate=BROKEN said="exit $status: $(sed -n '/^error/{p;q;}' "$log")" ;;
    esac
    if [[ $fate == BROKEN ]] || [[ $kernel == model && $fate != killed ]]; then
        lived=1
    elif [[ $kernel != model ]]; then
        fate=${fate/killed/found}
        fate=${fate/MISSED/-}
    fi
    printf '%-8s %-44s %-34s %s\n' "$fate" "$name" "$killer" "$said"
    rm -f "$log"
    rm -r "${copy%/"$model"}"
done
exit $lived
