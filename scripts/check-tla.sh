#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Model-check fxdax's TLA+ specs with TLC (tlaplus/tlaplus, MIT): the follower's protocol
# (specs/follower, backlog doc-8, TASK-42.8).
#
#   check-tla.sh [MODEL...]
#
# checks each model, by its configuration's name (specs/follower/MODEL.cfg, or
# mutations/MODEL.cfg): by default all of them. A model of the spec as designed must hold: TLC
# finds no violation. A mutation, the spec with one of its rules removed, must not: TLC must find
# the violation of the invariant its configuration names (`\* expect: NAME`), or of the temporal
# property it names and checks alone (`\* expect: property NAME`), or the check fails, as a check
# that never fails proves nothing.
#
# TLC runs on Java: $JAVA, or java if it runs, or Homebrew's openjdk. Its jar, the release pinned
# below, is fetched once into ~/.cache/fxdax/tla and checked against its SHA-256.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
specs="$root/specs/follower"
tla_version=1.8.0
tla_sha256=15d103b7e5617780bcc904c0af12f1ad242e2b345c0301136c5be4588f17edac
cache="$HOME/.cache/fxdax/tla"
jar="$cache/tla2tools-$tla_version.jar"

if [[ ! -f $jar ]]; then
    mkdir -p "$cache"
    curl -sSfL -o "$jar.part" \
        "https://github.com/tlaplus/tlaplus/releases/download/v$tla_version/tla2tools.jar"
    mv "$jar.part" "$jar"
fi
echo "$tla_sha256  $jar" | shasum -a 256 -c --status ||
    { echo "$jar does not match its SHA-256" >&2; exit 1; }

java=${JAVA:-}
if [[ -z $java ]]; then
    for candidate in java /opt/homebrew/opt/openjdk/bin/java /usr/local/opt/openjdk/bin/java; do
        if "$candidate" -version > /dev/null 2>&1; then
            java=$candidate
            break
        fi
    done
fi
[[ -n $java ]] || { echo "No Java runs: set JAVA" >&2; exit 2; }

models=("$@")
if (( ${#models[@]} == 0 )); then
    for cfg in "$specs"/*.cfg "$specs"/mutations/*.cfg; do
        models+=("$(basename "$cfg" .cfg)")
    done
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/fxdax-tla.XXXXXX")
trap 'rm -r "$work"' EXIT
status=0
for model in "${models[@]}"; do
    if [[ -f $specs/$model.cfg ]]; then
        cfg="$specs/$model.cfg"
        expect=
    elif [[ -f $specs/mutations/$model.cfg ]]; then
        cfg="$specs/mutations/$model.cfg"
        expect=$(sed -n 's/^\\\* expect: //p' "$cfg")
        [[ -n $expect ]] || { echo "$cfg names no expected violation" >&2; exit 2; }
    else
        echo "No model $model" >&2
        exit 2
    fi
    log="$work/$model.log"
    start=$SECONDS
    # TLC exits nonzero when it finds a violation, whose trace it prints rather than writing it
    # beside the spec (-noGenerateSpecTE).
    "$java" -XX:+UseParallelGC -cp "$jar" tlc2.TLC -workers auto -noGenerateSpecTE \
        -metadir "$work/$model.states" -config "$cfg" "$specs/Follower.tla" > "$log" 2>&1 &&
        found=0 || found=$?
    took=$((SECONDS - start))
    states=$(sed -n 's/.* \([0-9,]*\) distinct states found.*/\1/p' "$log" | tail -1)
    if [[ -z $expect ]]; then
        if (( found == 0 )) && grep -q '^Model checking completed. No error has been found' "$log"
        then
            echo "PASS $model: holds, $states distinct states, ${took}s"
        else
            echo "FAIL $model: TLC found a violation, or failed (exit $found):"
            sed -n '/^Error:/,$p' "$log" | sed -n 1,200p
            status=1
        fi
    elif [[ $expect == property\ * ]] &&
        grep -q "^Error: Temporal property ${expect#property } was violated" "$log"; then
        echo "PASS $model: violates ${expect#property } as expected, ${took}s"
    elif grep -q "^Error: Invariant $expect is violated" "$log"; then
        echo "PASS $model: violates $expect as expected, ${took}s"
    else
        echo "FAIL $model: expected a violation of $expect, but (exit $found):"
        grep -E '^Error|is violated|No error has been found' "$log" | sed -n 1,20p
        status=1
    fi
done
exit "$status"
