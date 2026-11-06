#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Generate the Go package extentdv1 (go/extentd/v1) from the extent daemon's .proto
# (crates/fxdax/proto/fxdax/extentd/v1/extentd.proto, TASK-219), as buf.gen.yaml says: into go/,
# or with --check, into a scratch directory, failing if what is checked in differs from it.
#
#   go/generate.sh [--check]
#
# buf compiles the .proto, in Go, as protox does for the Rust side: no protoc, whose version
# would be in every generated file. Its version is pinned here; the generators' by go.mod.
set -euo pipefail

BUF=github.com/bufbuild/buf/cmd/buf@v1.75.0
dir=$(cd "$(dirname "$0")" && pwd)
cd "$dir"

case ${1:-} in
"")
    go run "$BUF" generate
    ;;
--check)
    scratch=$(mktemp -d)
    trap 'rm -r "$scratch"' EXIT
    go run "$BUF" generate --output "$scratch"
    # Every generated file checked in as generated, and none checked in that nothing generates.
    status=0
    for generated in $(cd "$scratch" && find . -name '*.pb.go'); do
        diff -u "$generated" "$scratch/$generated" || status=1
    done
    for checked_in in $(find . -name '*.pb.go'); do
        [[ -e $scratch/$checked_in ]] || { echo "$checked_in: generated from no .proto" >&2; status=1; }
    done
    [[ $status == 0 ]] || echo "go/: generated code out of date with the .proto: run go/generate.sh" >&2
    exit "$status"
    ;;
*)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac
