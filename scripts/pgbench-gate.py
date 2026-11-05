#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Gates a GCE run of pgbench and of DAX's cases on throughput and memory.

The sandbox team's pgbench (TASK-131.5), and copy-on-write DAX's cases (TASK-164.5): DAX copies
on write, the only DAX since writable DAX was deleted (doc-13, dead-ends/writable-dax.md). Reads the
cases' logs of scripts/test-gce.sh results directories (e2e/CASE/CASE/log.txt, or each round's,
round-K/CASE/log.txt), each directory's rounds after the last's:

- pgbench's (test_fxfs_guest_pgbench.rs): their runs' tps in order, RUNS a VM as the guest's output
  beside the log says (stdout.txt's `OK runs RUNS`), as many VMs as one daemon served
  (FXDAX_PGBENCH_VMS). On the guest's Fxfs without a DAX window (fxfs-guest-pgbench: no-dax), or
  with one, its stores copying on write (-dax: dax), each VM making its database; on a database a
  base holds, each VM running on a fork of it (fxfs-guest-pgbench-forked, -dax-forked:
  no-dax-forked, dax-forked), its files read in, which DAX maps; so, each VM kept running until the
  last's runs are done, the host's memory measured as each is added (fxfs-guest-pgbench-density,
  -dax-density: no-dax-density, dax-density); and on ext4 with DAX on virtio-pmem, a static DAX
  device (pgbench-ext4-dax-pmem: ext4-dax-pmem).
- bigmap's (test_fxfs_guest_bigmap.rs): a file four times the guest's memory, in a base, mapped
  shared by a VM on a fork of it, read all over, 5% of it written, and synced: its results
  (`bigmap: NAME VALUE` on the console), without a DAX window (fxfs-guest-bigmap: bigmap-no-dax),
  or with one, its writes copying on write (-dax: bigmap-dax).
- the host's memory, which the density and bigmap cases say as their guests ask
  (`memory: vm=VM phase=PHASE NAME=VALUE...`, test_cases' measure.rs): MemTotal less MemAvailable
  (`host_used`), the VM process's, the daemon's, the guest's.

It writes them, and the gates, as JSON, and fails unless every gate passes. Each gate's value is at
least its threshold, or at most for those marked so. Copy-on-write DAX's were set from the first
run measured, GCE r632 (docs/measurements/dax-cow-gate.md): from its numbers, but for `dax` and
`forked-dax`, which hold doc-13's expectations.

- decay: each guest Fxfs pgbench case's last run of its first VM against its first run (the
  sandbox team saw ~21% lost over five), at least DECAY;
- dax: DAX against no DAX, each case's median run of its first VM: a database made
  in the run is files created in the mount, which take the page cache's path (doc-13), at least
  DAX;
- forked-dax: DAX against no DAX on a base's database, as `dax`: a fault on the host
  for each page's first store, nothing after (doc-13 expects 0.9 to 1), at least FORKED_DAX;
- static-dax: DAX against ext4 with DAX on pmem, as `dax`: the gap to a static DAX device, at least
  STATIC_DAX, by default 0, reported and not gated;
- density: of one daemon serving many VMs, the first round that had more than one, its last VM's
  median run against its first's, at least DENSITY;
- density-memory: each DAX density case's host memory per added VM against no DAX's,
  at most DENSITY_MEMORY;
- density-memory-mib: each DAX density case's host memory per added VM, in MiB, at
  most DENSITY_MEMORY_MIB;
- bigmap-read: each DAX bigmap case's cold random reads, MiB/s, against no DAX's, at
  least BIGMAP_READ;
- bigmap-guest-cache: each DAX bigmap case's guest page cache grown over its cold
  reads, a share of the file, at most BIGMAP_GUEST_CACHE: DAX reads cost the guest no memory;
- bigmap-written-memory: the DAX bigmap case's host memory grown over the writes,
  per byte written, at most BIGMAP_WRITTEN_MEMORY: only the written pages cost memory (doc-13).

A density case's host memory per added VM is its first round's host_used after each VM's runs, less
after the VM's before (or, for the first, before any VM): the mean over the VMs after the first,
which pays for what they share, the base's pages in the daemon.

Each pgbench run's guest latencies (TASK-111.6), the histograms the guest's Fxfs keeps of its
faults, seals and window MAPs (`NAME_hist` in /proc/fs/fxfs/extents, which the case's log has before
and after each run on its `fxfs stats:` lines), are reported as their percentiles, in us: the upper
bound of the bucket each falls in, bucket k's 1.024 us times 2 to the k, the last's none.

A gate whose cases didn't run is left out. The host's kind (`nested`, as GCE's n2 VMs are, or
`bare-metal`) is reported with the numbers, which differ much between the two (window remaps and
first-touch mappings cost more nested), never compared across.

Usage: scripts/pgbench-gate.py [--host nested|bare-metal] [--json PATH] RESULTS...

The thresholds: FXDAX_GATE_<GATE> (its name in capitals, dashes as underscores), or DEFAULTS'.
"""

import argparse
import json
import os
import re
import statistics
import sys
from pathlib import Path

# The pgbench cases, by the name the JSON gives them.
CASES = {
    "fxfs-guest-pgbench": "no-dax",
    "fxfs-guest-pgbench-dax": "dax",
    "pgbench-ext4-dax-pmem": "ext4-dax-pmem",
    "fxfs-guest-pgbench-forked": "no-dax-forked",
    "fxfs-guest-pgbench-dax-forked": "dax-forked",
    "fxfs-guest-pgbench-density": "no-dax-density",
    "fxfs-guest-pgbench-dax-density": "dax-density",
}
# Those of the guest's Fxfs, which its gates of decay and density hold to.
FXFS = tuple(name for name in CASES.values() if name != "ext4-dax-pmem")
# The gates of a case against another's median run of its first VM: the case and the other.
AGAINST = {
    "dax": ("dax", "no-dax"),
    "forked-dax": ("dax-forked", "no-dax-forked"),
    "static-dax": ("dax", "ext4-dax-pmem"),
}
# The DAX density cases, gated on their memory against no DAX's.
DENSITY_DAX = ("dax-density",)
# The bigmap cases, by the name the JSON gives them.
BIGMAP = {
    "fxfs-guest-bigmap": "bigmap-no-dax",
    "fxfs-guest-bigmap-dax": "bigmap-dax",
}
# The DAX bigmap cases, gated on their reads and memory.
BIGMAP_DAX = ("bigmap-dax",)
# The steps of a bigmap case at which the host measures, in order, the host before the VM first.
BIGMAP_PHASES = ("base", "mounted", "read-cold", "read-warm", "written", "synced", "end")
# A run's result, as pgbench prints it.
TPS = re.compile(r"^tps = ([0-9.]+) \(without initial connection time\)")
# The runs a VM makes, as its guest says once they are done (test_fxfs_guest_pgbench.rs).
RUNS = re.compile(r"^OK runs ([0-9]+)$")
# The guest's Fxfs's stats, names and values, on one line, as the case logs them around each run.
STATS = re.compile(r"^fxfs stats: (.*)$")
# The host's memory, as the host says it when a guest asks (measure.rs).
MEMORY = re.compile(r"^memory: (.*)$")
# A bigmap case's result (test_fxfs_guest_bigmap.rs).
RESULT = re.compile(r"^bigmap: ([a-z0-9_]+) (-?[0-9.]+)$")
# The percentiles of each run's latencies reported.
PERCENTILES = (50, 90, 99)
DEFAULTS = {
    "decay": 0.9,
    "dax": 0.95,
    "forked-dax": 0.9,
    "static-dax": 0.0,
    "density": 0.9,
    # Set from r632 (0.86, 2,361 MiB, 5.76, 0.0, 1.29), with room for a nested host's noise.
    "density-memory": 0.9,
    "density-memory-mib": 2560.0,
    "bigmap-read": 4.0,
    "bigmap-guest-cache": 0.05,
    "bigmap-written-memory": 1.5,
}
# The gates whose value is at most their threshold; every other's is at least its.
AT_MOST = frozenset({"density-memory", "density-memory-mib", "bigmap-guest-cache",
                     "bigmap-written-memory"})
MIB = 1 << 20


def records(lines):
    """Each pgbench run's record in `lines`, a case's log, in order: its tps, and its guest's
    latencies' percentiles (`latencies`); and the runs a VM made, if the guest said."""
    found, stats, per_vm = [], [], None
    for line in lines:
        if match := STATS.match(line):
            stats.append(match.group(1).split())
        elif match := TPS.match(line):
            # The run's stats are the last before its tps, and those before them, before it.
            between = latencies(stats[-2], stats[-1]) if len(stats) > 1 else {}
            found.append((float(match.group(1)), between))
        elif match := RUNS.match(line.strip()):
            per_vm = int(match.group(1))
    return found, per_vm


def by_vm(values, per_vm):
    """`values`, a value each run, VM by VM: `per_vm` runs each, or all one VM's."""
    per_vm = per_vm or len(values)
    return [values[at:at + per_vm] for at in range(0, len(values), per_vm)]


def runs(lines):
    """The tps of each VM's pgbench runs in `lines`, a case's log, VM by VM, in order."""
    found, per_vm = records(lines)
    return by_vm([tps for tps, _ in found], per_vm)


def latencies(before, after):
    """The percentiles of each histogram's counts between the stats `before` and `after` (each a
    stats line's words, names and values in turn), by the name before its `_hist`: how many, and
    each percentile's bucket's upper bound in us."""
    pairs = [dict(zip(words[::2], words[1::2])) for words in (before, after)]
    found = {}
    for name, value in pairs[1].items():
        if not name.endswith("_hist") or name not in pairs[0]:
            continue
        counts = [int(now) - int(then)
                  for now, then in zip(value.split(","), pairs[0][name].split(","))]
        found[name.removesuffix("_hist")] = percentiles(counts)
    return found


def percentiles(counts):
    """How many `counts`, a histogram's buckets, count, and each of PERCENTILES' bucket's upper
    bound in us: none for the last bucket, which has none, nor for no count."""
    total, found = sum(counts), {}
    for percentile in PERCENTILES:
        bound = None
        if total:
            reached, at = 0, 0
            for at, count in enumerate(counts):
                reached += count
                if reached * 100 >= percentile * total:
                    break
            bound = None if at == len(counts) - 1 else round(1.024 * 2 ** at, 3)
        found[f"p{percentile}_us"] = bound
    return {"count": total, **found}


def samples(lines):
    """The host's memory said in `lines`, a case's log, in order: each a dict of its `NAME=VALUE`
    words, `phase` a name, every other a number."""
    found = []
    for line in lines:
        if match := MEMORY.match(line.strip()):
            words = dict(word.split("=", 1) for word in match.group(1).split())
            found.append({name: value if name == "phase" else int(value)
                          for name, value in words.items()})
    return found


def results(lines):
    """A bigmap case's results said in `lines`, its log, by name."""
    return {match.group(1): float(match.group(2))
            for line in lines if (match := RESULT.match(line.strip()))}


def logs(results_dir, case):
    """The logs of `case` in `results_dir`, a test-gce.sh results directory: each round's, in
    order: its artifacts' (round-K/CASE/log.txt), or, of a round that passed, whose artifacts
    aren't kept, its whole log (round-K.log), which has its console and its guest's output both."""
    root = Path(results_dir) / "e2e" / case
    single = root / case / "log.txt"
    if single.exists():
        return [single]
    rounds = {}
    for path in root.glob("round-*.log"):
        rounds[int(path.stem.split("-")[1])] = path
    for path in root.glob(f"round-*/{case}/log.txt"):
        rounds[int(path.parent.parent.name.split("-")[1])] = path
    return [rounds[at] for at in sorted(rounds)]


def said(log):
    """The lines of a case's `log`, its console, where pgbench says its tps, then of its guest's
    output beside it (stdout.txt), where the guest says how many runs a VM made."""
    lines = log.read_text(errors="replace").splitlines()
    stdout = log.with_name("stdout.txt")
    if stdout.exists():
        lines += stdout.read_text(errors="replace").splitlines()
    return lines


def rounds_of(directories, case):
    """The lines of each round's log of `case` in the results `directories`, each directory's
    rounds after the last's."""
    return [said(log) for directory in directories for log in logs(directory, case)]


def measure(*directories):
    """Each pgbench case's rounds in the results `directories`, by its JSON name: each round's VMs'
    runs' tps, each directory's rounds after the last's."""
    return measure_each(directories, lambda found: [tps for tps, _ in found])


def measure_latencies(*directories):
    """As `measure`, each run's guest latencies' percentiles (`latencies`): of the cases whose
    guests keep them."""
    measured = measure_each(directories, lambda found: [between for _, between in found])
    return {name: rounds for name, rounds in measured.items()
            if any(run for vms in rounds for runs in vms for run in runs)}


def measure_each(directories, values):
    """Each pgbench case's rounds in the results `directories`, by its JSON name: each round's VMs'
    runs' `values`, of its runs' records (`records`), each directory's rounds after the last's."""
    measured = {}
    for case, name in CASES.items():
        rounds = []
        for lines in rounds_of(directories, case):
            found, per_vm = records(lines)
            rounds.append(by_vm(values(found), per_vm) if found else [])
        if any(rounds):
            measured[name] = [vms for vms in rounds if vms]
    return measured


def mib(value):
    """`value` bytes in MiB, to a tenth; None as None."""
    return None if value is None else round(value / MIB, 1)


def density_memory(memory):
    """What a density case's VMs cost the host, of its first round's `memory` (`samples`): what
    the first VM cost (`first_vm_mib`), the mean of what each after it did (`per_added_vm_mib`),
    and the host's, each VM process's, the daemon's and each guest's memory after each VM's runs;
    or None, without the host's memory before the first VM and after one."""
    before = [sample for sample in memory if sample["vm"] == 0]
    ran = sorted((sample for sample in memory if sample["vm"] > 0), key=lambda s: s["vm"])
    if not before or not ran:
        return None
    used = [before[0]["host_used"]] + [sample["host_used"] for sample in ran]
    added = [now - then for then, now in zip(used, used[1:])]

    def column(name):
        return [mib(sample.get(name)) for sample in ran]

    summary = {
        "vms": len(ran),
        "first_vm_mib": mib(added[0]),
        "per_added_vm_mib": mib(statistics.mean(added[1:])) if len(added) > 1 else None,
        "host_used_mib": [mib(value) for value in used],
        "vmm_pss_mib": column("vmm_pss"),
        "vmm_anon_mib": column("vmm_anon"),
        "vmm_shmem_mib": column("vmm_shmem"),
        "extentd_memory_mib": column("extentd_memory_bytes"),
        "guest_used_mib": [mib(sample["guest_total"] - sample["guest_available"])
                           if "guest_total" in sample else None for sample in ran],
        "guest_cached_mib": column("guest_cached"),
    }
    # Copy-on-write DAX's private pages, as the daemon or the guest counts them, if they do.
    for name in sorted({name for sample in ran for name in sample if "private" in name}):
        summary[name] = [sample.get(name) for sample in ran]
    return summary


def bigmap_summary(said_results, memory):
    """A bigmap case's `said_results` (`results`), with what its steps cost, of its `memory`
    (`samples`): the guest's page cache grown over the cold reads, a share of the file
    (`guest_cache_share`); the host's memory grown over them and over the writes, in MiB, and over
    the writes per byte written (`written_memory_per_byte`); the host's, the VM process's and the
    daemon's at each step, against the host's before the VM; and the private pages counted at each.
    None without every step."""
    at = {sample["phase"]: sample for sample in memory}
    if any(phase not in at for phase in BIGMAP_PHASES) or any(
            name not in said_results for name in ("file_bytes", "read_cold_mib_s", "write_pages")):
        return None

    def grown(name, then, now):
        return at[now][name] - at[then][name]

    written = said_results["write_pages"] * 4096
    summary = dict(said_results)
    summary.update({
        "guest_cache_share": round(grown("guest_cached", "mounted", "read-cold")
                                   / said_results["file_bytes"], 4),
        "host_read_mib": mib(grown("host_used", "mounted", "read-cold")),
        "host_written_mib": mib(grown("host_used", "read-warm", "written")),
        "written_memory_per_byte": round(grown("host_used", "read-warm", "written") / written, 4),
        "host_used_mib": {phase: mib(grown("host_used", "base", phase))
                          for phase in BIGMAP_PHASES[1:]},
        "vmm_pss_mib": {phase: mib(at[phase].get("vmm_pss")) for phase in BIGMAP_PHASES[1:]},
        "extentd_memory_mib": {phase: mib(at[phase].get("extentd_memory_bytes"))
                               for phase in BIGMAP_PHASES},
    })
    # Copy-on-write DAX's private pages, as the daemon or the guest counts them, if they do.
    for name in sorted({name for sample in memory for name in sample if "private" in name}):
        summary[name] = {phase: at[phase].get(name) for phase in BIGMAP_PHASES}
    return summary


def measure_memory(*directories):
    """Each density case's host memory (`density_memory`), and each bigmap case's results and
    what its steps cost (`bigmap_summary`), by its JSON name, each of its first round that has
    them."""
    density, bigmap = {}, {}
    for case, name in CASES.items():
        for lines in rounds_of(directories, case):
            if summary := density_memory(samples(lines)):
                density[name] = summary
                break
    for case, name in BIGMAP.items():
        for lines in rounds_of(directories, case):
            if summary := bigmap_summary(results(lines), samples(lines)):
                bigmap[name] = summary
                break
    return density, bigmap


def judge(name, case, value, thresholds):
    """Gate `name` of `case`, of `value`, against its threshold in `thresholds`: at most it, for
    those of AT_MOST, at least it for the others."""
    threshold, most = thresholds[name], name in AT_MOST
    return {"gate": name, "case": case, "value": round(value, 4), "threshold": threshold,
            "bound": "at most" if most else "at least",
            "passed": value <= threshold if most else value >= threshold}


def gates(measured, thresholds, density=None, bigmap=None):
    """The gates of `measured`, as `measure` has it, and of `density` and `bigmap`, as
    `measure_memory` has them, against `thresholds`, by gate (`judge`). Of each pgbench case's
    first round, but density's, of its first round of more than one VM."""
    found = []
    first = {case: rounds[0] for case, rounds in measured.items()}
    for case in FXFS:
        if case not in measured:
            continue
        vms = first[case]
        found.append(judge("decay", case, vms[0][-1] / vms[0][0], thresholds))
        dense = next((vms for vms in measured[case] if len(vms) > 1), None)
        if dense:
            value = statistics.median(dense[-1]) / statistics.median(dense[0])
            found.append(judge("density", case, value, thresholds))
    for name, (case, other) in AGAINST.items():
        if case in first and other in first:
            value = statistics.median(first[case][0]) / statistics.median(first[other][0])
            found.append(judge(name, case, value, thresholds))
    density, bigmap = density or {}, bigmap or {}
    no_dax = (density.get("no-dax-density") or {}).get("per_added_vm_mib")
    for case in DENSITY_DAX:
        per_vm = (density.get(case) or {}).get("per_added_vm_mib")
        if per_vm is None:
            continue
        if no_dax:
            found.append(judge("density-memory", case, per_vm / no_dax, thresholds))
        found.append(judge("density-memory-mib", case, per_vm, thresholds))
    for case in BIGMAP_DAX:
        if case not in bigmap:
            continue
        if "bigmap-no-dax" in bigmap:
            value = bigmap[case]["read_cold_mib_s"] / bigmap["bigmap-no-dax"]["read_cold_mib_s"]
            found.append(judge("bigmap-read", case, value, thresholds))
        found.append(judge("bigmap-guest-cache", case, bigmap[case]["guest_cache_share"],
                           thresholds))
        found.append(judge("bigmap-written-memory", case,
                           bigmap[case]["written_memory_per_byte"], thresholds))
    return found


def thresholds():
    """The gates' thresholds: the environment's FXDAX_GATE_<GATE>, or their defaults."""
    return {name: float(os.environ.get(f"FXDAX_GATE_{name.upper().replace('-', '_')}", default))
            for name, default in DEFAULTS.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--host", choices=["nested", "bare-metal"], default="nested")
    parser.add_argument("--json")
    parser.add_argument("results", nargs="+")
    args = parser.parse_args()
    measured = measure(*args.results)
    density, bigmap = measure_memory(*args.results)
    if not measured and not bigmap:
        raise SystemExit(f"no pgbench runs in {' '.join(args.results)}")
    found = gates(measured, thresholds(), density, bigmap)
    report = {"host": args.host, "tps": measured, "gates": found}
    if latency := measure_latencies(*args.results):
        report["latency"] = latency
    if density:
        report["memory"] = density
    if bigmap:
        report["bigmap"] = bigmap
    text = json.dumps(report, indent=2)
    if args.json:
        Path(args.json).write_text(text + "\n")
    print(text)
    failed = [gate for gate in report["gates"] if not gate["passed"]]
    for gate in failed:
        side = "over" if gate["bound"] == "at most" else "under"
        print(f"failed: {gate['gate']} of {gate['case']}: {gate['value']} {side} "
              f"{gate['threshold']}", file=sys.stderr)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
