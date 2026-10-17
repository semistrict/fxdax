#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Lists the fxdax code whose upstream Fuchsia sources changed.

The code copied, ported or excerpted from Fuchsia follows the revision in the root Cargo.toml's
`workspace.metadata.fuchsia.commit`. It cites its sources as backquoted paths from the root of the
Fuchsia tree, such as `zircon/kernel/vm/page_queues.cc`: a Rust file in its leading comment, and a
crate copied from Fuchsia in its Cargo.toml's. This finds the sources that changed between that
revision and REVISION in a Fuchsia checkout, and lists each file citing them, so the changes can be
brought over; then record REVISION as the commit in Cargo.toml. A source that was renamed or moved
is listed under its old path, which is the one cited.

Usage: scripts/upstream-changes.py [--diff] REVISION
       scripts/upstream-changes.py --check

REVISION   the Fuchsia revision to compare with, such as origin/main
--diff     also print the upstream changes
--check    only check that every cited source exists at the recorded revision

The Fuchsia checkout is $FUCHSIA, by default ~/src/fuchsia; REVISION must be fetched there.
"""

import argparse
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FUCHSIA = Path(os.environ.get("FUCHSIA", Path.home() / "src" / "fuchsia"))
CITATION = re.compile(r"`((?:zircon|src|sdk|third_party|prebuilt)/[^`\s]+)`")


def git(directory, *args, check=True):
    return subprocess.run(
        ["git", "-C", str(directory), *args], check=check, capture_output=True, text=True
    )


def recorded_commit():
    with open(ROOT / "Cargo.toml", "rb") as manifest:
        return tomllib.load(manifest)["workspace"]["metadata"]["fuchsia"]["commit"]


def leading_comment(path, marker):
    """Returns the comments before a file's first line of code, such as a copyright notice and the
    comment following it, in a file whose comments start with `marker`."""
    lines = []
    for line in (ROOT / path).read_text().splitlines():
        if line and not line.startswith(marker):
            break
        lines.append(line)
    return "\n".join(lines)


def citations():
    """Maps each Rust file and crate manifest under crates/ to the upstream paths it cites."""
    files = git(ROOT, "ls-files", "--cached", "--others", "--exclude-standard", "--", "crates")
    cited = {}
    for path in files.stdout.splitlines():
        if not (ROOT / path).exists():
            continue
        if path.endswith(".rs"):
            paths = CITATION.findall(leading_comment(path, "//"))
        elif path.endswith("/Cargo.toml"):
            paths = CITATION.findall(leading_comment(path, "#"))
        else:
            continue
        if paths:
            cited[path] = sorted(set(paths))
    return cited


def covers(citation, path):
    return path == citation or path.startswith(citation + "/")


def check(cited, commit):
    missing = [
        (path, citation)
        for path, paths in cited.items()
        for citation in paths
        if git(FUCHSIA, "cat-file", "-e", f"{commit}:{citation}", check=False).returncode != 0
    ]
    for path, citation in missing:
        print(f"{path}: cites `{citation}`, which is not in Fuchsia at {commit}")
    return 1 if missing else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("revision", nargs="?")
    parser.add_argument("--diff", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.check and not args.revision:
        parser.error("REVISION is required")

    commit = recorded_commit()
    cited = citations()
    if args.check:
        return check(cited, commit)

    # Without rename detection, a renamed source shows as its old path deleted, which is cited.
    changes = git(
        FUCHSIA, "diff", "--name-only", "--no-renames", commit, args.revision
    ).stdout.splitlines()
    for path, paths in sorted(cited.items()):
        sources = [c for c in changes if any(covers(citation, c) for citation in paths)]
        if not sources:
            continue
        print(path)
        for source in sources:
            print(f"    {source}")
        if args.diff:
            diff = git(
                FUCHSIA, "--no-pager", "diff", "--no-renames", commit, args.revision, "--", *sources
            )
            print(diff.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
