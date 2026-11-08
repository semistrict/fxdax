#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Reports the pgbench cases' runs step by step, as the sandbox team's harness does (TASK-91.8).

Reads each pgbench case's log under the results directories given (test_fxfs_guest_pgbench.rs's,
`.../CASE/CASE/log.txt`, as scripts/test-gce.sh and scripts/bench-gce.sh keep them), and its
daemon's samples beside it (samples.txt, which the host writes every 2 s of the loading VM's life,
the store cases' alone). For each case:

- the load: `pgbench -i`'s time and its phases, as pgbench says them ("done in ... (drop tables
  ..., create tables ..., client-side generate ..., vacuum ..., primary keys ...)");
- each run: its tps and mean latency, as pgbench says them; the guest's CPU over it, user (with
  nice), system (with irq and softirq), idle (with iowait) and steal, shares of all its vCPUs' time,
  from the guest's /proc/stat said before and after it; and the extent daemon's requests over it,
  by kind, a second and their mean time, and its commits' mean wait for object storage, from the
  samples bracketing the run (the last at or before its start, the first at or after its end, by
  the guest's uptime, which the samples' clock, started as the VM boots, follows).

Writes Markdown to standard output, and with --json, the same as JSON to the file it names.
"""

import argparse
import json
import re
import sys
from pathlib import Path

# A command the guest ran, and when, by its uptime: "pgbench: at 16.48 s: COMMAND".
STARTED = re.compile(r"^pgbench: at ([0-9.]+) s: (.*)$")
# How long it took: "pgbench: 60.1s, exit status: 0" (Rust's Duration, in s or ms).
TOOK = re.compile(r"^pgbench: ([0-9.]+)(ms|s), exit status")
# The guest's CPU time so far, in ticks: user nice system idle iowait irq softirq steal ...
CPU = re.compile(r"^guest cpu: cpu\s+(.*)$")
# A run of pgbench's transactions, its options before it if any.
RUN = re.compile(r"(^|\s)pgbench -h \S+ -c [0-9]+ -j [0-9]+ -T [0-9]+ ")
# pgbench -i's summary, and each of its phases.
LOADED = re.compile(r"^done in ([0-9.]+) s \((.*)\)\.$")
PHASE = re.compile(r"^(.+) ([0-9.]+) s$")
TPS = re.compile(r"^tps = ([0-9.]+) \(without initial connection time\)")
LATENCY = re.compile(r"^latency average = ([0-9.]+) ms")
# A sample's first line, by the seconds since the VM booted.
SAMPLE = re.compile(r"^t=([0-9]+) ")


def numbers(words):
    """The `NAME=VALUE` words of a line, those whose values are numbers, by name."""
    found = {}
    for word in words:
        name, _, value = word.partition("=")
        if value.isdigit():
            found[name] = int(value)
    return found


def samples(path):
    """The daemon's samples in `path`, in order: each its time `t`, its `daemon` counters and its
    `requests` by kind, each kind a count and its total ns."""
    found = []
    for line in path.read_text(errors="replace").splitlines():
        if match := SAMPLE.match(line):
            found.append({"t": int(match.group(1)), "daemon": {}, "requests": {}})
        elif found and line.startswith("daemon: "):
            found[-1]["daemon"] = numbers(line.split()[1:])
        elif found and line.startswith("requests: "):
            found[-1]["requests"] = numbers(line.split()[1:])
    return found


def bracketing(found, start, end):
    """The samples of `found` around [`start`, `end`]: the last at or before `start`, and the
    first at or after `end`; none if either is missing."""
    before = [sample for sample in found if sample["t"] <= start]
    after = [sample for sample in found if sample["t"] >= end]
    if not before or not after:
        return None
    return before[-1], after[0]


def daemon_over(found, start, end):
    """The extent daemon's work between the samples of `found` around [`start`, `end`]: its
    requests by kind, a second and their mean us, and its commits' mean wait for object storage,
    in us; none without samples around it."""
    around = bracketing(found, start, end)
    if around is None:
        return None
    before, after = around
    seconds = after["t"] - before["t"]
    requests = {}
    for name, count in after["requests"].items():
        if name.endswith("-ns"):
            continue
        made = count - before["requests"].get(name, 0)
        if made <= 0:
            continue
        ns = after["requests"].get(f"{name}-ns", 0) - before["requests"].get(f"{name}-ns", 0)
        requests[name] = {"per_s": round(made / seconds, 1), "mean_us": round(ns / made / 1000, 1)}
    commits = after["daemon"].get("commits", 0) - before["daemon"].get("commits", 0)
    waited = after["daemon"].get("commit-wait-ns", 0) - before["daemon"].get("commit-wait-ns", 0)
    wait = round(waited / commits / 1000, 1) if commits > 0 else None
    return {"seconds": seconds, "requests": requests, "commits_per_s": round(commits / seconds, 1),
            "commit_wait_us": wait}


def cpu_over(before, after):
    """The guest's CPU between its /proc/stat ticks `before` and `after`, as shares of all its
    time: user (with nice), system (with irq and softirq), idle (with iowait) and steal."""
    delta = [now - then for now, then in zip(after, before)]
    total = sum(delta[:8]) or 1
    user, nice, system, idle, iowait, irq, softirq, steal = delta[:8]
    share = lambda ticks: round(ticks / total, 3)
    return {"user": share(user + nice), "system": share(system + irq + softirq),
            "idle": share(idle + iowait), "steal": share(steal)}


def phases(said):
    """pgbench -i's phases, as it says them ("drop tables 0.00 s, ..."), in seconds by name."""
    return {match.group(1): float(match.group(2))
            for phase in said.split(", ") if (match := PHASE.match(phase))}


def steps(log, found):
    """The load and runs of the case whose log's lines are `log`, its daemon's samples `found`."""
    load, runs, cpu, run = None, [], None, None
    for line in log:
        if match := STARTED.match(line):
            if RUN.search(match.group(2)):
                run = {"started": float(match.group(1)), "cpu_before": cpu}
        elif (match := TOOK.match(line)) and run is not None and "seconds" not in run:
            took = float(match.group(1)) / (1000 if match.group(2) == "ms" else 1)
            run["seconds"] = took
        elif match := CPU.match(line):
            cpu = [int(word) for word in match.group(1).split()]
            if run is not None and "seconds" in run and "cpu" not in run:
                run["cpu"] = cpu_over(run.pop("cpu_before"), cpu) if run["cpu_before"] else None
        elif match := LOADED.match(line):
            load = {"seconds": float(match.group(1)), "phases": phases(match.group(2))}
        elif (match := LATENCY.match(line)) and run is not None:
            run["latency_ms"] = float(match.group(1))
        elif (match := TPS.match(line)) and run is not None:
            run["tps"] = float(match.group(1))
            run.pop("cpu_before", None)
            run["daemon"] = daemon_over(found, run["started"], run["started"] + run["seconds"])
            runs.append(run)
            run = None
    return {"load": load, "runs": runs}


def cases(directories):
    """Each pgbench case's steps under `directories`, by its name: the logs of a case's directory
    named as the directory it is in (`.../CASE/CASE/log.txt`), those of a VM case's own."""
    found = {}
    for directory in directories:
        for log in sorted(Path(directory).rglob("log.txt")):
            case = log.parent.name
            if log.parent.parent.name != case or "pgbench" not in case:
                continue
            lines = log.read_text(errors="replace").splitlines()
            stdout = log.with_name("stdout.txt")
            if stdout.exists():
                lines += stdout.read_text(errors="replace").splitlines()
            sampled = log.with_name("samples.txt")
            found[case] = steps(lines, samples(sampled) if sampled.exists() else [])
    return found


def percent(share):
    return "–" if share is None else f"{share * 100:.0f}%"


def markdown(found):
    """`found`, as `cases` has it, in Markdown: a summary of every case, then each case's runs."""
    out = ["| Case | Load | " + " | ".join(f"Run {n + 1} tps" for n in range(runs(found))) + " |",
           "|---|---|" + "---|" * runs(found)]
    for case, steps_ in found.items():
        load = steps_["load"]
        cells = [f"{run['tps']:,.0f}" for run in steps_["runs"]]
        cells += ["–"] * (runs(found) - len(cells))
        out.append(f"| {case} | {load['seconds']:.2f} s | " + " | ".join(cells) + " |"
                   if load else f"| {case} | – | " + " | ".join(cells) + " |")
    for case, steps_ in found.items():
        out += ["", f"## {case}", ""]
        if load := steps_["load"]:
            said = ", ".join(f"{name} {seconds:.2f} s" for name, seconds in load["phases"].items())
            out += [f"Load (pgbench -i): {load['seconds']:.2f} s: {said}.", ""]
        out += ["| Run | tps | Latency | Guest user | System | Idle | Steal | "
                "extentd requests/s (mean us) | Commits/s (mean wait) |",
                "|---|---|---|---|---|---|---|---|---|"]
        for n, run in enumerate(steps_["runs"], 1):
            cpu = run.get("cpu") or {}
            daemon = run.get("daemon")
            if daemon:
                made = ", ".join(f"{name} {kind['per_s']:,.0f} ({kind['mean_us']:,.0f})"
                                 for name, kind in sorted(daemon["requests"].items(),
                                                          key=lambda item: -item[1]["per_s"]))
                wait = daemon["commit_wait_us"]
                waited = f"{daemon['commits_per_s']:,.0f}"
                waited += "" if wait is None else f" ({wait:,.0f} us)"
            else:
                made, waited = "–", "–"
            latency = run.get("latency_ms")
            out.append(f"| {n} | {run['tps']:,.0f} | "
                       f"{'–' if latency is None else f'{latency:.2f} ms'} | "
                       f"{percent(cpu.get('user'))} | {percent(cpu.get('system'))} | "
                       f"{percent(cpu.get('idle'))} | {percent(cpu.get('steal'))} | {made} | "
                       f"{waited} |")
    return "\n".join(out) + "\n"


def runs(found):
    """The most runs any case made."""
    return max((len(steps_["runs"]) for steps_ in found.values()), default=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--json")
    parser.add_argument("results", nargs="+")
    args = parser.parse_args()
    found = cases(args.results)
    if not found:
        raise SystemExit(f"no pgbench case's log in {' '.join(args.results)}")
    if args.json:
        Path(args.json).write_text(json.dumps(found, indent=2) + "\n")
    sys.stdout.write(markdown(found))


if __name__ == "__main__":
    main()
