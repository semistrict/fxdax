#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Finds the guest stores an extent daemon's DAX windows refused that fail the run (TASK-44.12).

The window copies on write (doc-13): a vCPU's store to a page mapped read-only gets a private copy,
and the window refuses a store to a page it maps nothing at, or past private_max, which stops the
VM: the guest and the window disagreeing (TASK-99). But KVM's asynchronous page fault, a kernel
worker's, faulting a page in for a vCPU as its guest goes on, may find it refused too, of a page
the window just remapped, which the guest was reading: all 2,327 of r422's refused stores and all
28 of r394's (TASK-150.16). So, given the VM processes' logs, which name the thread, only a store
a vCPU made fails the run, whatever the page: a worker's is no store of the guest's, whose own
access decides. The logs hold each first thousand refusals and every power of two after, and a
vCPU's stays refused until its VM stops, so that one past that shows. Given the daemon's log
alone, which can't say whose a store was, every store it refused fails the run.

Usage: scripts/dax-refusals.py EXTENTD_LOG [FIRECRACKER_LOG...]

prints each refused store that fails the run, as logged (its first line), and exits 1 if there are
any.
"""

import re
import sys

# The daemon's line for a store a window refused (`ExtentStore::refused`), with what the window
# maps at the page and the extent's state in the store: not the line of what a refusal met later.
REFUSED = re.compile(r"extentd: a store at DAX window offset (0x[0-9a-f]+) refused")


# A VM process's line for a store the window refused, which names the thread that made it, and
# says whether it is one of the VMM's vCPUs, by the VMM's own registry: a jailed VMM has no /proc to
# name a thread by (TASK-167). Lines from before it name the thread alone.
THREAD = re.compile(r"the DAX window refused a store .*, by thread \d+ \(([^,)]*)(?:, ([^)]*))?\)")


def refused(lines):
    """The lines of `lines`, the daemon's, of stores refused: whose, it can't say."""
    return [line for line in lines if REFUSED.search(line)]


def by_guests(lines):
    """The lines of `lines`, a VM process's, of stores refused that a vCPU made: no kernel worker,
    as KVM's asynchronous page faults' are."""
    threads = ((line, THREAD.search(line)) for line in lines)
    return [line for line, thread in threads if thread and guests(thread[1], thread[2])]


def guests(name, whose):
    """Whether a store refused of thread `name`, which the VMM says is `whose` (or doesn't), was
    the guest's own."""
    if whose is not None:
        return whose == "a vCPU"
    return not name.startswith("kworker")


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    found = []
    if len(sys.argv) == 2:
        with open(sys.argv[1], errors="replace") as log:
            found = refused(log)
    for name in sys.argv[2:]:
        with open(name, errors="replace") as log:
            found += by_guests(log)
    for line in found:
        print(line.rstrip("\n"))
    sys.exit(1 if found else 0)


if __name__ == "__main__":
    main()
