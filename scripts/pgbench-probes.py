#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""What a pgbench run's guest says of DAX's write path, and whether it got worse (TASK-146.3).

Reads the last `fxfs stats:` line of a pgbench run's console, what /proc/fs/fxfs said at its end,
and prints the mean of each probe: a window MAP request and a write(2). Fails if one is past its ceiling: half as much again as measured on 2026-10-31 (GCE r365,
default commits, against which a host's variation is well within that).

Usage: scripts/pgbench-probes.py CONSOLE
"""

import re
import sys

# Each probe: its name, the counters of its nanoseconds and of how many, and its ceiling in us.
PROBES = [
    ("window MAP request", "window_maps_ns", "window_map_requests", 560),
    ("write(2)", "dax_writes_ns", "dax_writes", 560),
]


def stats(console):
    """The counters of `console`'s last `fxfs stats:` line."""
    lines = re.findall(r"fxfs stats: (.*)", console)
    if not lines:
        raise ValueError("no `fxfs stats:` line")
    words = lines[-1].split()
    return dict(zip(words[::2], words[1::2]))


def probes(counters):
    """Each probe's name, mean in us and ceiling."""
    means = []
    for name, nanoseconds, count, ceiling in PROBES:
        if int(counters[count]) == 0:
            raise ValueError(f"no {count}: nothing measured")
        means.append((name, int(counters[nanoseconds]) / int(counters[count]) / 1000, ceiling))
    return means


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1], errors="replace") as console:
        means = probes(stats(console.read()))
    for name, mean, ceiling in means:
        print(f"{name}: {mean:.0f} us (ceiling {ceiling})")
    past = [name for name, mean, ceiling in means if mean > ceiling]
    if past:
        sys.exit(f"past their ceilings: {', '.join(past)}")


if __name__ == "__main__":
    main()
