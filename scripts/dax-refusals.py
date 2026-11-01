#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Finds the guest stores an extent daemon's DAX windows refused that no seal explains (TASK-44.12).

A guest's store to a page of the extent being sealed may reach the window after the seal made the
page read-only, before the guest's own seal makes its mappings read-only: KVM's asynchronous page
fault, a kernel worker's, faults the page in for the vCPU as its guest goes on, and the window
refuses it; the guest's task retries once the guest made its mapping read-only, through its own
fault, which maps the page anew. Every DAX run has hundreds such, of pages of extents sealed. A
store refused to a page of an extent still open, or mapped writably, is the guest and the window
disagreeing (TASK-99): that fails the run. But the same worker's fault refused on a page the window
just remapped, which the guest was reading, looks like that to the daemon too, and it was all 2,327
of r422's refused stores and all 28 of r394's (TASK-150.16): given the VM processes' logs, which name
the thread, only a store a vCPU made fails the run, whatever the page: a worker's is no store of the
guest's, whose own access decides. The logs hold each first thousand refusals and every power of two
after, and a vCPU's stays refused until its VM stops, so that one past that shows.

Usage: scripts/dax-refusals.py EXTENTD_LOG [FIRECRACKER_LOG...]

prints each refused store that fails the run, as logged (its first line), and exits 1 if there are
any.
"""

import re
import sys

# The daemon's line for a store a window refused (`ExtentStore::refused`): what the window maps at
# the page, and the extent's state in the store.
REFUSED = re.compile(r"extentd: a store at DAX window offset (0x[0-9a-f]+) refused")
# A seal's: the page mapped read-only, of an extent neither open in the window nor in the store,
# which holds it sealed, or has freed it since, as a follower's windows go on mapping it.
SEALED = re.compile(
    r", read-only; not open in the window; in the store: "
    r"Ok\((Some\(\"0x[0-9a-f]+ bytes, sealed\"\)|None)\)"
)


# A VM process's line for a store the window refused, which names the thread that made it.
THREAD = re.compile(r"the DAX window refused a store .*, by thread \d+ \(([^)]*)\)")


def unexplained(lines):
    """The lines of `lines` that say of a store refused that no seal explains."""
    return [line for line in lines if REFUSED.search(line) and not SEALED.search(line)]


def by_guests(lines):
    """The lines of `lines`, a VM process's, of stores refused that no kernel worker made."""
    threads = ((line, THREAD.search(line)) for line in lines)
    return [line for line, thread in threads if thread and not thread[1].startswith("kworker")]


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    found = []
    if len(sys.argv) == 2:
        with open(sys.argv[1], errors="replace") as log:
            found = unexplained(log)
    for name in sys.argv[2:]:
        with open(name, errors="replace") as log:
            found += by_guests(log)
    for line in found:
        print(line.rstrip("\n"))
    sys.exit(1 if found else 0)


if __name__ == "__main__":
    main()
