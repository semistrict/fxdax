#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compares two sides of an A/B of the fs bench (scripts/bench-ab-host.sh), each one or more of its
results directories, as rounds alternating between the two make them: each workload's values
pooled across a side's directories and cases, their medians and ranges, how many times slower B
is than A (above 1, B is slower), and whether the two ranges overlap, which a difference within
noise does. It prints a Markdown table.

With --control CASE, each case's workloads are compared apart, and against the control: CASE, a
case whose code is the same on both sides (ext4 on virtio-blk, as `ext4-guest-kernel`), whose own
B-vs-A ratio for the same workload is the drift between the sides' runs. A difference is called
real only where the two sides' ranges don't overlap and it is larger than the control's drift for
that workload (TASK-276): the rounds interleave the sides on one host, and what still moves the
control moves the others as much.

Without one, it says too which workloads moved, their ranges apart, and lists those one side
alone measured. And each side's failures: of each kind, how many of its directories (its rounds)
had one, and what each said, if they say (a qualification's checks, scripts/qualify.sh, TASK-330.4).

Usage: scripts/bench-ab.py [--control CASE] A_NAME A_DIR... -- B_NAME B_DIR...
"""

import json
import math
import statistics
import sys
from pathlib import Path


def pooled(directories):
    """Each workload's values across `directories`' results, and its unit and direction."""
    values, kinds = {}, {}
    for directory in directories:
        for path in sorted(Path(directory).glob("bench-*.json")):
            for result in json.loads(path.read_text())["results"]:
                values.setdefault(result["workload"], []).extend(result["values"])
                kinds[result["workload"]] = (result["unit"], result["higher_is_better"])
    return values, kinds


def by_case(directories):
    """Each case's workloads' values across `directories`' results, by (case, workload), the case
    its backend, and each workload's unit and direction."""
    values, kinds = {}, {}
    for directory in directories:
        for path in sorted(Path(directory).glob("bench-*.json")):
            report = json.loads(path.read_text())
            case = report.get("backend", path.stem)
            for result in report["results"]:
                values.setdefault((case, result["workload"]), []).extend(result["values"])
                kinds[result["workload"]] = (result["unit"], result["higher_is_better"])
    return values, kinds


def slower(a, b, higher_is_better):
    """How many times slower B's values are than A's, by their medians: above 1, B is slower."""
    a_median, b_median = statistics.median(a), statistics.median(b)
    return a_median / b_median if higher_is_better else b_median / a_median


def overlap(a, b):
    return max(min(a), min(b)) <= min(max(a), max(b))


def verdict(ratio, overlaps, control):
    """Whether a difference of `ratio` is real: the ranges apart, and further from 1 than the
    control's own ratio, `control` (None if the control has no such workload)."""
    if control is None:
        return "no control"
    if overlaps or abs(math.log(ratio)) <= abs(math.log(control)):
        return "within drift"
    return "real"


def control_table(a_name, a_dirs, b_name, b_dirs, control):
    (a, kinds), (b, _) = by_case(a_dirs), by_case(b_dirs)
    lines = [
        f"| Case | Workload | Unit | {a_name} | {b_name} | {b_name} vs {a_name} | "
        f"{control} {b_name} vs {a_name} | Verdict |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for (case, workload), a_values in sorted(a.items()):
        b_values = b.get((case, workload))
        if not b_values or case == control:
            continue
        unit, higher_is_better = kinds[workload]
        ratio = slower(a_values, b_values, higher_is_better)
        a_control, b_control = a.get((control, workload)), b.get((control, workload))
        drift = None
        if a_control and b_control:
            drift = slower(a_control, b_control, higher_is_better)
        lines.append(
            f"| {case} | {workload} | {unit} | {described(a_values)} | {described(b_values)} | "
            f"{ratio:.2f}× | {'-' if drift is None else f'{drift:.2f}×'} | "
            f"{verdict(ratio, overlap(a_values, b_values), drift)} |"
        )
    return "\n".join(lines)


def described(values):
    return f"{statistics.median(values):,.4g} ({min(values):,.4g}–{max(values):,.4g})"


def table(a_name, a_dirs, b_name, b_dirs):
    (a, kinds), (b, b_kinds) = pooled(a_dirs), pooled(b_dirs)
    lines = [
        f"| Workload | Unit | {a_name} | {b_name} | {b_name} vs {a_name} | Ranges overlap |",
        "|---|---|---|---|---|---|",
    ]
    # A's workloads, then those B alone measured.
    kinds.update((workload, kind) for workload, kind in b_kinds.items() if workload not in kinds)
    for workload, (unit, higher_is_better) in kinds.items():
        if workload not in a or workload not in b:
            lines.append(
                f"| {workload} | {unit} | {described(a[workload]) if workload in a else '-'} | "
                f"{described(b[workload]) if workload in b else '-'} | - | - |"
            )
            continue
        a_median, b_median = statistics.median(a[workload]), statistics.median(b[workload])
        slower = a_median / b_median if higher_is_better else b_median / a_median
        overlap = max(min(a[workload]), min(b[workload])) <= min(max(a[workload]), max(b[workload]))
        lines.append(
            f"| {workload} | {unit} | {described(a[workload])} | {described(b[workload])} | "
            f"{slower:.2f}× | {'yes' if overlap else 'no'} |"
        )
    return "\n".join(lines)


def moved(a_dirs, b_dirs):
    """The workloads both sides measured whose ranges are apart."""
    (a, _), (b, _) = pooled(a_dirs), pooled(b_dirs)
    return [workload for workload in a if workload in b and not overlap(a[workload], b[workload])]


def failures(directories):
    """Of each kind of failure in `directories`' results, how many of the directories had one; and
    what each failure said, by directory."""
    rounds, said = {}, []
    for directory in directories:
        kinds = set()
        for path in sorted(Path(directory).glob("bench-*.json")):
            report = json.loads(path.read_text())
            kinds.update(kind for kind, count in report.get("failures", {}).items() if count)
            said.extend(f"{Path(directory).name}: {text}" for text in report.get("failed", []))
        for kind in kinds:
            rounds[kind] = rounds.get(kind, 0) + 1
    return rounds, said


def failure_table(a_name, a_dirs, b_name, b_dirs):
    """Each side's failures (`failures`), a table of their kinds, then what each said; empty if
    neither side failed."""
    (a, a_said), (b, b_said) = failures(a_dirs), failures(b_dirs)
    if not a and not b:
        return ""
    lines = [f"| Failed | {a_name} | {b_name} |", "|---|---|---|"]
    for kind in sorted(set(a) | set(b)):
        lines.append(f"| {kind} | {a.get(kind, 0)} of {len(a_dirs)} | "
                     f"{b.get(kind, 0)} of {len(b_dirs)} |")
    said = a_said + b_said
    if said:
        lines += [""] + [f"- {text}" for text in said]
    return "\n".join(lines)


def main():
    args = sys.argv[1:]
    control = None
    if args[:1] == ["--control"] and len(args) > 1:
        control, args = args[1], args[2:]
    if "--" not in args or args.index("--") < 2 or len(args) - args.index("--") < 3:
        sys.exit(__doc__.strip().splitlines()[-1])
    cut = args.index("--")
    a_name, a_dirs, b_name, b_dirs = args[0], args[1:cut], args[cut + 1], args[cut + 2 :]
    if control:
        print(control_table(a_name, a_dirs, b_name, b_dirs, control))
    else:
        print(table(a_name, a_dirs, b_name, b_dirs))
        print(f"\nMoved, their ranges apart: {', '.join(moved(a_dirs, b_dirs)) or 'none'}.")
    failed = failure_table(a_name, a_dirs, b_name, b_dirs)
    if failed:
        print(f"\n{failed}")


if __name__ == "__main__":
    main()
