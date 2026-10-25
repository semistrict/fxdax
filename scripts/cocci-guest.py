#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs the semantic patches in scripts/cocci, the shapes the guest's DAX bugs had (TASK-44.17),
over the guest's C, with Coccinelle's spatch, and prints what each matches: any match fails, so
that the next instance of a shape is found as it is written, not by a VM stopping.

    scripts/cocci-guest.py [SOURCE...]

checks SOURCE, files or directories, or the guest's C (guest/linux/fs/fxfs and
guest/linux/drivers/virtio_extent). Each rule's matches print as spatch's context diffs, whose
lines starting with '-' are the code matched.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RULES = ROOT / "scripts/cocci"
SOURCES = [ROOT / "guest/linux/fs/fxfs", ROOT / "guest/linux/drivers/virtio_extent"]


def matches(rule, source):
    """What `rule` matches in `source`, a file or a directory: spatch's context diff, or ''."""
    where = ["--dir", str(source)] if Path(source).is_dir() else [str(source)]
    try:
        ran = subprocess.run(
            ["spatch", "--very-quiet", "--sp-file", str(rule), *where],
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        raise SystemExit("no spatch: install Coccinelle (apt install coccinelle, brew install "
                         "coccinelle)") from None
    if ran.returncode != 0:
        raise SystemExit(f"spatch {rule.name} on {source} failed:\n{ran.stdout}{ran.stderr}")
    return ran.stdout


def main():
    sources = [Path(source) for source in sys.argv[1:]] or SOURCES
    failed = False
    for rule in sorted(RULES.glob("*.cocci")):
        for source in sources:
            found = matches(rule, source)
            if found:
                print(f"{rule.name} matches in {source}:\n{found}", flush=True)
                failed = True
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
