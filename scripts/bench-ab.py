#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compares two sides of an A/B of the fs bench (scripts/bench-gce.sh), each one or more of its
results directories, as rounds alternating between the two make them: each workload's values
pooled across a side's directories and cases, their medians and ranges, how many times slower B
is than A (above 1, B is slower), and whether the two ranges overlap, which a difference within
noise does. It prints a Markdown table.

Usage: scripts/bench-ab.py A_NAME A_DIR... -- B_NAME B_DIR...
"""

import json
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


def described(values):
    return f"{statistics.median(values):,.4g} ({min(values):,.4g}–{max(values):,.4g})"


def table(a_name, a_dirs, b_name, b_dirs):
    (a, kinds), (b, _) = pooled(a_dirs), pooled(b_dirs)
    lines = [
        f"| Workload | Unit | {a_name} | {b_name} | {b_name} vs {a_name} | Ranges overlap |",
        "|---|---|---|---|---|---|",
    ]
    for workload, (unit, higher_is_better) in kinds.items():
        if workload not in b:
            continue
        a_median, b_median = statistics.median(a[workload]), statistics.median(b[workload])
        slower = a_median / b_median if higher_is_better else b_median / a_median
        overlap = max(min(a[workload]), min(b[workload])) <= min(max(a[workload]), max(b[workload]))
        lines.append(
            f"| {workload} | {unit} | {described(a[workload])} | {described(b[workload])} | "
            f"{slower:.2f}× | {'yes' if overlap else 'no'} |"
        )
    return "\n".join(lines)


def main():
    args = sys.argv[1:]
    if "--" not in args or args.index("--") < 2 or len(args) - args.index("--") < 3:
        sys.exit(__doc__.strip().splitlines()[-1])
    cut = args.index("--")
    print(table(args[0], args[1:cut], args[cut + 1], args[cut + 2 :]))


if __name__ == "__main__":
    main()
