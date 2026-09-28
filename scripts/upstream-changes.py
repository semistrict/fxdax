#!/usr/bin/env python3
"""Lists the fxdax code whose upstream Fuchsia sources changed.

Code ported or mirrored from Fuchsia cites its sources in its leading comment,
as backquoted paths from the root of the Fuchsia tree, such as
`zircon/kernel/vm/page_queues.cc`. This finds the vendored sources that
changed, by default the uncommitted changes under vendor/fuchsia that
scripts/vendor-fuchsia.sh leaves, and lists each file citing them, so the
changes can be ported. Code built directly from vendor/fuchsia needs no
porting.

Usage: scripts/upstream-changes.py [--diff] [REVISION]
       scripts/upstream-changes.py --check

REVISION   compare vendor/fuchsia with this fxdax revision instead of HEAD,
           such as the commit before a vendor update
--diff     also print the upstream changes
--check    only check that every cited source exists in vendor/fuchsia
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENDOR = "vendor/fuchsia"
CITATION = re.compile(r"`((?:zircon|src|sdk|third_party|prebuilt)/[^`\s]+)`")


def git(*args):
    return subprocess.run(
        ["git", "-C", str(ROOT), *args], check=True, capture_output=True, text=True
    ).stdout.splitlines()


def leading_comment(path):
    """Returns the comments before a file's first line of code, such as a copyright notice and the
    comment following it."""
    lines = []
    for line in (ROOT / path).read_text().splitlines():
        if line and not line.startswith("//"):
            break
        lines.append(line)
    return "\n".join(lines)


def citations():
    """Maps each source file under crates/ to the upstream paths it cites."""
    files = git("ls-files", "--cached", "--others", "--exclude-standard", "--", "crates")
    cited = {}
    for path in files:
        if path.endswith(".rs") and (ROOT / path).exists():
            paths = CITATION.findall(leading_comment(path))
            if paths:
                cited[path] = sorted(set(paths))
    return cited


def changed(revision):
    """Returns the vendored paths, relative to vendor/fuchsia, that changed since `revision`."""
    paths = git("diff", "--name-only", revision, "--", VENDOR)
    paths += git("ls-files", "--others", "--exclude-standard", "--", VENDOR)
    return sorted(p.removeprefix(VENDOR + "/") for p in paths)


def covers(citation, path):
    return path == citation or path.startswith(citation + "/")


def check(cited):
    missing = [
        (path, citation)
        for path, paths in cited.items()
        for citation in paths
        if not (ROOT / VENDOR / citation).exists()
    ]
    for path, citation in missing:
        print(f"{path}: cites `{citation}`, which is not in {VENDOR}")
    return 1 if missing else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("revision", nargs="?", default="HEAD")
    parser.add_argument("--diff", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    cited = citations()
    if args.check:
        return check(cited)

    changes = changed(args.revision)
    uncited = set(changes)
    for path, paths in sorted(cited.items()):
        sources = [c for c in changes if any(covers(citation, c) for citation in paths)]
        if not sources:
            continue
        uncited -= set(sources)
        print(path)
        for source in sources:
            print(f"    {source}")
        if args.diff:
            vendored = [f"{VENDOR}/{source}" for source in sources]
            subprocess.run(["git", "-C", str(ROOT), "--no-pager", "diff", args.revision, "--", *vendored])
    if changes:
        print(f"{len(uncited)} of {len(changes)} changed vendored files are not cited by ported code.")
    else:
        print(f"No changes under {VENDOR} since {args.revision}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
