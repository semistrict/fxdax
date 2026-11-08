#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Says how much of guest/sim CI runs: `full`, if what it tests changed since the base, else `smoke`.

guest/sim's seeds (zloop.sh, 3,000 of them) and its sweep (sweep.sh, every call of every run of
one failing in turn) took half an hour each, CI's wall time twice its other jobs'; since 2026-10-31
the seeds failed eight times in 430 runs, each beside the workspace's failures of the same rewrite,
and the sweep never (the user's decision, 2026-11-08). So CI runs them whole only when the code
they test changed, dax.c and its kernel (guest/linux/fs/fxfs, the kernel's patches and how it is
built) or the simulator; otherwise a smoke run, and the nightly workflow runs them whole.

The base is what CI compares with: a pull request's base, a push's commit before it, or, if
neither is given (a dispatch, a branch's first push), the merge-base with origin/main. One that
isn't in the history, as after a force push, is full. FXDAX_GUEST_SIM, `full` or `smoke`, as a
dispatch of CI may ask, says instead.

Usage: scripts/guest-sim-scope.py [BASE [HEAD]]
"""

import os
import subprocess
import sys

# What guest/sim tests, directories and files: a change of one runs it whole.
TESTED = (
    "guest/linux/fs/fxfs/",
    "guest/linux/patches/",
    "guest/sim/",
    "scripts/build-guest-kernel.sh",
)
NO_COMMIT = "0" * 40


def tested(path):
    """Whether `path` is what guest/sim tests: in a directory of `TESTED`, or a file of it."""
    return any(path.startswith(it) if it.endswith("/") else path == it for it in TESTED)


def scope(paths):
    """`full` if any of `paths`, changed, is what guest/sim tests; else `smoke`."""
    return "full" if any(tested(path) for path in paths) else "smoke"


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True)


def changed(base, head):
    """The paths changed from `base` to `head`, or None if `base` isn't a commit here."""
    if git("cat-file", "-e", f"{base}^{{commit}}").returncode != 0:
        return None
    diff = git("diff", "--name-only", f"{base}...{head}")
    if diff.returncode != 0:
        return None
    return diff.stdout.split()


def main(args):
    asked = os.environ.get("FXDAX_GUEST_SIM", "")
    if asked in ("full", "smoke"):
        print(asked)
        return 0
    base = args[0] if args else ""
    head = args[1] if len(args) > 1 else "HEAD"
    if base in ("", NO_COMMIT):
        merge_base = git("merge-base", "origin/main", head)
        base = merge_base.stdout.strip() if merge_base.returncode == 0 else ""
    paths = changed(base, head) if base else None
    print("full" if paths is None else scope(paths))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
