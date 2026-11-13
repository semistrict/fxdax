# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced by the mutant scripts (crates/*/mutants.sh): whether a mutant's patch applies as it is,
# the same on every host, run from the repository's root. A patch applies with no fuzz, as GNU
# patch takes it (`-F0`), and BSD patch (macOS) is laxer: it takes a hunk whose header counts other
# lines than it has, and one with more context on one side of its changes than the other away from
# the file's ends, which GNU patch then anchors to the file's start or end. A patch re-anchored by
# hand on macOS checked there and failed on ubu1 (TASK-395).

# patch_well_formed PATCH: whether each of its hunks has the lines its header counts, and as many
# lines of context after its changes as before, but at the start or end of the file it changes.
patch_well_formed() {
    awk '
        function fail(msg) {
            printf "%s: %s\n", FILENAME, msg > "/dev/stderr"
            bad = 1
        }
        function close_hunk() {
            if (!hunk) return
            hunk = 0
            if (old > 0 || new > 0) {
                fail("the hunk at line " at " has fewer lines than its header counts")
            } else if (lead != trail && !(lead < trail && first == 1) &&
                       !(trail < lead && first + count - 1 == lines)) {
                fail("the hunk at line " at " has " lead " lines of context before its changes and " \
                     trail " after, away from the start and end of " file)
            }
        }
        !hunk && /^\+\+\+ / {
            file = $2
            sub(/^b\//, "", file)
            lines = 0
            while ((getline line < file) > 0) lines++
            close(file)
            after = 0
            next
        }
        /^@@ / {
            close_hunk()
            split($2, o, ",")
            split($3, n, ",")
            first = substr(o[1], 2) + 0
            count = (2 in o) ? o[2] + 0 : 1
            old = count
            new = (2 in n) ? n[2] + 0 : 1
            hunk = 1
            after = 1
            at = FNR
            lead = trail = changed = 0
            next
        }
        hunk && /^[ +-]/ {
            c = substr($0, 1, 1)
            if (c == " ") {
                old--
                new--
                if (changed) trail++; else lead++
            } else {
                changed = 1
                trail = 0
                if (c == "-") old--; else new--
            }
            if (old < 0 || new < 0) {
                fail("the hunk at line " at " has more lines than its header counts")
                hunk = 0
            } else if (old == 0 && new == 0) {
                close_hunk()
            }
            next
        }
        hunk && /^\\/ { next }
        hunk { close_hunk() }
        after && /^[ +-]/ && !/^(---|\+\+\+) / {
            fail("line " FNR " is past its hunk, as its header counts it")
        }
        END {
            close_hunk()
            exit bad
        }
    ' "$1"
}

# patch_round_trip PATCH: whether it is well formed, and applies and reverses, leaving the files it
# changes as they were, in a copy of them.
patch_round_trip() {
    local copy file status=0
    patch_well_formed "$1" || return 1
    copy=$(mktemp -d)
    for file in $(sed -n 's|^+++ b/||p' "$1"); do
        mkdir -p "$copy/$(dirname "$file")"
        cp "$file" "$copy/$file"
    done
    if ! patch -s -F0 -p1 -d "$copy" < "$1" > /dev/null; then
        echo "$1 doesn't apply" >&2
        status=1
    elif ! patch -s -F0 -R -p1 -d "$copy" < "$1" > /dev/null; then
        echo "$1 doesn't reverse" >&2
        status=1
    else
        for file in $(sed -n 's|^+++ b/||p' "$1"); do
            cmp -s "$file" "$copy/$file" || { echo "$1: $file isn't as it was, reversed" >&2; status=1; }
        done
    fi
    rm -r "$copy"
    return $status
}
