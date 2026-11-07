#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Checks that what the tests leave out says why, and that a why naming a task is still open.

What a test or suite leaves out outlives its reason unless something looks (TASK-149.20: a guest
check covering a data loss bug had been switched off on DAX). So each of these says why:

- An ignored Rust test of fxdax's: `#[ignore = "why"]`, not a bare `#[ignore]`, nor a cfg_attr
  ignoring it with none. A file of Fuchsia's (its copyright Fuchsia's) keeps upstream's ways.
- A line of the open bugs' lists of failures the runs let be (scripts/pjdfstest-fuse-known,
  guest/sim/known-failures, crates/fxfs-posix/known-panics): its task first, which is open.
- xfstests' known failures (guest/tests/test_cases/src/test_xfstests.rs, KNOWN_FAILURES): each
  under a comment saying why, a task it names open.
- pjdfstest's skipped tests on the guest's Fxfs (test_pjdfstest.rs, FXFS_SKIPPED): a task their why
  names open.
- A hand mutant no test can kill (guest/mutants, `Killer: none`): its header says why, equivalent
  or out of every oracle's reach, as guest/mutants/README.md has it.

A task is open unless backlog/tasks has it Done, or it is archived. scripts/test.sh's scripts part
runs this, as CI does. Exits 1, saying each, if anything breaks these.

Usage: scripts/check-exclusions.py
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The lists of open bugs' failures, a line each: the task, then the failure.
KNOWN_LISTS = [
    "scripts/pjdfstest-fuse-known",
    "guest/sim/known-failures",
    "crates/fxfs-posix/known-panics",
]
XFSTESTS = "guest/tests/test_cases/src/test_xfstests.rs"
PJDFSTEST = "guest/tests/test_cases/src/test_pjdfstest.rs"
MUTANTS = "guest/mutants"
# Where fxdax's Rust tests are: not vendor/, Firecracker's and the guest kernel's sources.
RUST = ["crates", "guest/tests", "guest/rust"]

TASK = re.compile(r"\bTASK-\d+(?:\.\d+)*\b")
BARE_IGNORE = re.compile(r"#\[ignore\]|#\[cfg_attr\([^]]*,\s*ignore\s*\)\]")
FUCHSIA = re.compile(r"Copyright \d{4} The Fuchsia Authors")


def task_states(root):
    """Each task's status, by ID (TASK-1.2): backlog/tasks' as their front matter says; an archived
    task's "Archived"."""
    states = {}
    for directory, archived in (("tasks", False), ("completed", True), ("archive", True)):
        for path in sorted((root / "backlog" / directory).rglob("*.md")):
            front = path.read_text(errors="replace").split("---")
            if len(front) < 3:
                continue
            fields = dict(
                line.split(":", 1) for line in front[1].splitlines() if ":" in line
            )
            task = fields.get("id", "").strip().strip("'\"").upper()
            if task:
                status = fields.get("status", "").strip().strip("'\"")
                states[task] = "Archived" if archived else status
    return states


def closed(text, states):
    """The tasks `text` names that aren't open: Done, archived or unknown, each with why."""
    found = []
    for task in TASK.findall(text):
        state = states.get(task)
        if state is None:
            found.append(f"{task}, no such task")
        elif state in ("Done", "Archived"):
            found.append(f"{task}, {state.lower()}")
    return found


def bare_ignores(path, text):
    """The lines of `text`, a Rust file at `path`, that ignore a test without saying why."""
    if FUCHSIA.search("\n".join(text.splitlines()[:5])):
        return []
    return [
        f"{path}:{number}: an ignored test says no why: #[ignore = \"why\"]"
        for number, line in enumerate(text.splitlines(), 1)
        if BARE_IGNORE.search(line)
    ]


def known_list(path, text, states):
    """What is wrong with `text`, a list of open bugs' failures at `path`: lines naming no task
    first, or a task not open."""
    problems = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        first = line.split()[0]
        if not TASK.fullmatch(first):
            problems.append(f"{path}:{number}: names no task first: {line}")
        for task in closed(first, states):
            problems.append(f"{path}:{number}: its task isn't open ({task}): drop the line")
    return problems


def xfstests_known(path, text, states):
    """What is wrong with KNOWN_FAILURES in `text`, xfstests' case at `path`: a failure under no
    comment saying why, or a comment naming a task not open."""
    match = re.search(r"const KNOWN_FAILURES: \[&str; \d+\] = \[(.*?)\n\s*\];", text, re.S)
    if not match:
        return [f"{path}: no KNOWN_FAILURES"]
    start = text[: match.start(1)].count("\n") + 1
    problems, why = [], None
    for offset, line in enumerate(match.group(1).splitlines()):
        number, line = start + offset, line.strip()
        if line.startswith("//"):
            why = line if why is None else f"{why} {line}"
            for task in closed(line, states):
                problems.append(f"{path}:{number}: a known failure's task isn't open ({task})")
        elif line.startswith('"'):
            if why is None:
                problems.append(f"{path}:{number}: {line.rstrip(',')} is under no comment saying why")
        elif not line:
            why = None
    return problems


def pjdfstest_skipped(path, text, states):
    """What is wrong with FXFS_SKIPPED in `text`: a why naming a task not open."""
    match = re.search(r"FXFS_SKIPPED: &\[\(&str, &str\)\] = &\[(.*?)\];", text, re.S)
    if not match:
        return [f"{path}: no FXFS_SKIPPED"]
    return [f"{path}: a skipped test's task isn't open ({task})" for task in closed(match.group(1), states)]


def unkillable(path, text):
    """What is wrong with the hand mutant at `path`, `text`: no killer, its header saying no why."""
    header = text.split("\n---", 1)[0]
    if not re.search(r"^# Killer: none\s*$", header, re.M):
        return []
    if re.search(r"quivalent|oracle", header):
        return []
    return [f"{path}: Killer: none, and its header says no why (equivalent, or out of every oracle's reach)"]


def check(root):
    """Every problem of the tree at `root`, each a line."""
    states = task_states(root)
    problems = []
    for directory in RUST:
        for path in sorted((root / directory).rglob("*.rs")):
            if "target" in path.relative_to(root).parts:
                continue
            problems += bare_ignores(path.relative_to(root), path.read_text(errors="replace"))
    for name in KNOWN_LISTS:
        problems += known_list(name, (root / name).read_text(), states)
    problems += xfstests_known(XFSTESTS, (root / XFSTESTS).read_text(), states)
    problems += pjdfstest_skipped(PJDFSTEST, (root / PJDFSTEST).read_text(), states)
    for path in sorted((root / MUTANTS).glob("*.patch")):
        problems += unkillable(path.relative_to(root), path.read_text())
    return problems


def main():
    problems = check(ROOT)
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"exclusions: {len(problems)} saying no why, or why no longer")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
