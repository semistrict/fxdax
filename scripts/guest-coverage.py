#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Reports what the guest checks ran of fs/fxfs, from a coverage kernel's counts (TASK-119.14).

A kernel built with FXDAX_GUEST_KERNEL_BUILD=coverage counts each VM's runs of fs/fxfs, which
fxfs-guest-checks leaves under FXDAX_COVERAGE, a directory of .gcda files for each VM. They are
merged, and gcov reads them against what the kernel's build left.

Usage: scripts/guest-coverage.py COUNTS OBJECTS REPORT

COUNTS is FXDAX_COVERAGE, OBJECTS the kernel's built fs/fxfs, and REPORT a new directory, which
holds gcov's annotated sources (dax.c.gcov and so on) and coverage.txt: for each source, its lines
and branches run, the functions never called, and the lines never run in the others. gcov looks
for sources as the kernel's build names them, from the top of its tree: REPORT links to them.
"""

import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

SOURCES = ("dax", "device", "super")

FUNCTION = re.compile(r"^function (\S+) called (\d+)")
LINE = re.compile(r"^\s*(#####|=====|[\d*]+):\s*(\d+):")
BRANCH = re.compile(r"^branch\s+\d+ (never executed|taken (\d+))")


def summarize(gcov):
    """What the text `gcov` of a source's gcov -b -c says: its counts and what never ran."""
    report = {"lines": 0, "run": 0, "branches": 0, "taken": 0, "never_called": [], "unrun": {}}
    unrun = defaultdict(list)
    function = None
    for line in gcov.splitlines():
        if found := FUNCTION.match(line):
            function = found[1]
            if int(found[2]) == 0:
                report["never_called"].append(function)
        elif found := BRANCH.match(line):
            report["branches"] += 1
            report["taken"] += bool(found[2]) and int(found[2]) > 0
        elif found := LINE.match(line):
            report["lines"] += 1
            if found[1] in ("#####", "====="):
                unrun[function].append(int(found[2]))
            else:
                report["run"] += 1
    never_called = set(report["never_called"])
    report["unrun"] = {name: lines for name, lines in unrun.items() if name not in never_called}
    return report


def spans(lines):
    """`lines`, sorted numbers, as text naming their runs: 1-3,7."""
    runs = []
    for line in lines:
        if runs and line == runs[-1][1] + 1:
            runs[-1][1] = line
        else:
            runs.append([line, line])
    return ",".join(str(a) if a == b else f"{a}-{b}" for a, b in runs)


def describe(name, report):
    """The lines of coverage.txt of source `name`."""
    percent = lambda part, whole: f"{100 * part / whole:.1f}%" if whole else "none"
    lines = [
        f"{name}.c: lines {report['run']}/{report['lines']} "
        f"({percent(report['run'], report['lines'])}), branches taken "
        f"{report['taken']}/{report['branches']} "
        f"({percent(report['taken'], report['branches'])})"
    ]
    lines += [f"  never called: {function}" for function in report["never_called"]]
    lines += [f"  {function}: not run: {spans(unrun)}" for function, unrun in report["unrun"].items()]
    return lines


def merged(counts, into):
    """The .gcda files of the VMs' directories under `counts`, merged in the directory `into`."""
    vms = sorted(path for path in counts.iterdir() if path.is_dir())
    if not vms:
        raise SystemExit(f"{counts} holds no VM's counts")
    shutil.copytree(vms[0], into)
    for vm in vms[1:]:
        subprocess.run(["gcov-tool", "merge", "-o", into, into, vm], check=True)


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    counts, objects, report = map(Path, sys.argv[1:])
    report.mkdir()
    counted = report / "counts"
    merged(counts, counted)
    for tree in ("arch", "fs", "include"):
        (report / tree).symlink_to(objects.resolve().parent.parent / tree)
    text = []
    for name in SOURCES:
        shutil.copy(objects / f"{name}.gcno", counted)
        subprocess.run(
            ["gcov", "-b", "-c", "-o", counted.resolve(), f"{name}.c"],
            cwd=report,
            check=True,
            stdout=subprocess.DEVNULL,
        )
        text += describe(name, summarize((report / f"{name}.c.gcov").read_text()))
    (report / "coverage.txt").write_text("\n".join(text) + "\n")


if __name__ == "__main__":
    main()
