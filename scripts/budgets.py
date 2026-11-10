#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""What each extent daemon of a run cost, against what its design bounds it by (TASK-330.2): the
budgets a run of guest tests fails on, as scripts/test-e2e.sh runs this over its results, and
scripts/bench.sh's runs with it, so that a change that trades one cost for another fails here,
not at a customer (LangSmith's sandboxes found TASK-310's doubled writes and TASK-317's memory).

Of each daemon's stats file (FXDAX_EXTENTD_STATS: the run's extentd-stats.txt, a case's daemon's
NAME-stats.txt), its last lines:

- **memory**: no sample of the daemon's anonymous memory past its budget, which the daemon weighs
  each against as it takes it (`process:`'s `over-budget`, fxdax::extentd::process): the sum of
  what its design bounds, its uploads' memory, a framing for each store written at once, its
  fetches', its tenants' rooms' and its own;
- **uploads**: what they held at once within their budget, the host's upload memory
  (`uploads:`, TASK-317);
- **disk**: what the daemon's process had its kernel write to its disk (`io_write_bytes`, every
  page as it dirtied it) no more than what it says it wrote: its stores' files, by kind
  (`written_*_bytes`), each write a page more at most (`written_writes`), its stats file's pages
  (`stats_written_bytes`), its log, the file beside the stats file, its bucket's objects, if a file
  bucket on a disk (`object-store:`'s `up-*`, and what was in flight as it stopped, its upload
  memory at most), and the parts it fetched into its cache
  (`daemon:`'s `fetched-bytes`): a write the daemon makes twice, uncounted, as TASK-310's
  write-behind was suspected of, passes it.

And says, of each, what it wrote by kind for each byte its guests wrote (`written_raw_bytes`, every
guest byte once), and its CPU time a request, which a workload's own case holds to its design
(`fxdax::remote::write_budgets`, of a host's store; the guest cases' own).

    budgets.py [--bucket-in-memory STATS]... RESULTS    each daemon's under RESULTS; exits 1 past one

`--bucket-in-memory` names a stats file whose daemon's bucket is in memory, on tmpfs, as
scripts/test-e2e.sh's own is: none of its objects is written to a disk. Writes budgets.json to
RESULTS, each daemon's checks and figures.
"""

import argparse
import json
import sys
from pathlib import Path

PAGE = 4096


def fields(path):
    """The last stats `path` keeps, by line: `KIND: NAME=COUNT ...` as {KIND: {NAME: COUNT}}."""
    kept = {}
    for line in path.read_text(errors="replace").splitlines():
        kind, _, counts = line.partition(": ")
        kept[kind] = {name: int(count) for name, _, count in
                      (word.partition("=") for word in counts.split()) if count.isdigit()}
    return kept


def log_of(stats):
    """The log of the daemon whose stats file is `stats`: NAME.log beside NAME-stats.txt, a case's
    daemon's, or extentd.log beside extentd-stats.txt, the run's."""
    return stats.with_name(stats.name.removesuffix("-stats.txt") + ".log")


def check(stats, bucket_in_memory=False):
    """The checks of the daemon whose stats file is `stats`: (budget, passed, what it says), and its
    figures."""
    kept = fields(stats)
    process, uploads = kept.get("process", {}), kept.get("uploads", {})
    daemon, store = kept.get("daemon", {}), kept.get("object-store", {})
    checks, figures = [], {}
    mib = lambda count: f"{count / (1 << 20):.1f} MiB"
    if "over-budget" in process:
        over = process["over-budget"]
        checks.append(("memory", over == 0,
                       f"anonymous memory {mib(process['max-anon'])} at most, budget "
                       f"{mib(process['anon-budget'])}; {over} samples past it, by "
                       f"{mib(process['max-over-budget'])} at most"))
        figures.update({"max_anon": process["max-anon"], "anon_budget": process["anon-budget"],
                        "max_shmem": process["max-shmem"], "cpu_ns": process["cpu-ns"]})
    if "max-held" in uploads:
        held, budget = uploads["max-held"], uploads["budget"]
        checks.append(("uploads", held <= budget,
                       f"uploads held {mib(held)} at most, budget {mib(budget)}"))
    kinds = ["raw", "copied", "framed", "own", "log"]
    if all(f"written_{kind}_bytes" in daemon for kind in kinds) and "written_writes" in daemon:
        written = {kind: daemon[f"written_{kind}_bytes"] for kind in kinds}
        log = log_of(stats)
        allowed = {
            "stores": sum(written.values()),
            "pages": daemon["written_writes"] * PAGE,
            "stats": daemon.get("stats_written_bytes", 0),
            "log": log.stat().st_size if log.exists() else 0,
            # Its objects uploaded, and those whose PUTs it had in flight as it stopped, killed, as
            # a case's daemon is: no more than its uploads hold at once, their budget.
            "bucket": 0 if bucket_in_memory else
            sum(count for name, count in store.items() if name.startswith("up-"))
            + uploads.get("budget", 0),
            "cache": daemon.get("fetched-bytes", 0),
        }
        io = daemon.get("io_write_bytes", 0)
        bound = sum(allowed.values())
        checks.append(("disk", io <= bound,
                       f"the kernel wrote {mib(io)} for it, it says {mib(bound)} at most: "
                       + ", ".join(f"{name} {mib(count)}" for name, count in allowed.items())))
        raw = written["raw"]
        figures["written"] = written
        figures["io_write_bytes"] = io
        if raw:
            figures["per_guest_byte"] = {kind: round(count / raw, 3) for kind, count in
                                         written.items()}
            figures["per_guest_byte"]["io"] = round(io / raw, 3)
    requests = sum(count for name, count in kept.get("requests", {}).items()
                   if not name.endswith("-ns"))
    if requests and "cpu-ns" in process:
        figures["cpu_ns_per_request"] = process["cpu-ns"] // requests
    return checks, figures


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bucket-in-memory", action="append", default=[], type=Path)
    parser.add_argument("results", type=Path)
    args = parser.parse_args(argv)
    in_memory = {path.resolve() for path in args.bucket_in_memory}
    report, failed = [], False
    for stats in sorted(args.results.rglob("*-stats.txt")):
        checks, figures = check(stats, stats.resolve() in in_memory)
        name = str(stats.relative_to(args.results))
        for budget, passed, says in checks:
            print(f"{name}: {budget}: {'ok' if passed else 'PAST ITS BUDGET'}: {says}",
                  file=sys.stderr)
            failed |= not passed
        if "per_guest_byte" in figures:
            per = " ".join(f"{kind} {count}" for kind, count in figures["per_guest_byte"].items())
            print(f"{name}: written for each guest byte: {per}", file=sys.stderr)
        report.append({"daemon": name, "checks": [{"budget": budget, "passed": passed,
                                                   "says": says} for budget, passed, says in checks],
                       "figures": figures})
    (args.results / "budgets.json").write_text(json.dumps(report, indent=1) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
