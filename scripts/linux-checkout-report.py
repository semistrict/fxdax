#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tables, in Markdown, of what the linux-checkout cases measured (TASK-136.1,
guest/tests/test_cases/src/test_linux_checkout.rs): a checkout the size of Linux's on ext4 and on
the guest's Fxfs, with and without DAX, and shared as a volume over a read-only base.

    scripts/linux-checkout-report.py RESULTS

RESULTS is scripts/test-e2e.sh's results directory (a GCE run's `e2e`), with a directory for each
case run: `linux-checkout-ext4`, `linux-checkout-ext4-pmem`, `linux-checkout-fxfs` and
`linux-checkout-fxfs-dax`. Each case's
guests say `measure ROLE NAME VALUE UNIT` on its standard output (stdout.txt) and the host
`memory: vm=N phase=ROLE-STAGE NAME=VALUE...` on its log (log.txt), as crate::measure has it."""

import argparse
import re
import sys
from pathlib import Path

CASES = ["ext4", "ext4-pmem", "fxfs", "fxfs-dax"]
MIB = 1 << 20
MEASURE = re.compile(r"^measure (\S+) (\S+) (\S+) (\S+)$")

# The timings of the VM that checks Linux out and of the one that opens it fresh, in order.
CHECKOUT = ["mkfs", "default-mount", "mount", "clone", "checkout", "sync", "status-cold",
            "status-warm", "status-synced", "status-warm-again", "read-cold", "read-warm",
            "publish"]
# A sandbox's timings, over a shared volume.
SANDBOX = ["attach", "import", "mount", "status-cold", "status-warm", "status-synced",
           "status-warm-again", "read-cold", "read-warm", "first-overwrite", "first-create",
           "status-after-writes"]
HUNDREDS = ["import-median", "import-max", "mount-median", "mount-max", "status-cold",
            "status-warm", "status-synced", "first-overwrite", "first-create", "unmount-all"]
REBOOTED = ["default-mount", "mount", "status-cold", "status-warm", "mount-others-median",
            "mount-others-max", "unmount-all"]
# What the host's disk holds that the checkout puts there: stores, buckets and the daemon's part
# cache, not logs nor the root filesystem.
DISK = re.compile(r"^(bucket|extentd-cache|generation|publisher|publisher\.bucket|sandbox-\d+|"
                  r"hundreds|ext4\.img)$")


def parse(stdout, log):
    """What a case said: its guests' and host's measurements, `{role: {name: (value, unit)}}`, and
    the host's memory lines, `[(phase, {name: value})]`, from the texts `stdout` and `log`."""
    measured = {}
    for line in stdout.splitlines():
        match = MEASURE.match(line.strip())
        if match:
            role, name, value, unit = match.groups()
            measured.setdefault(role, {})[name] = (float(value), unit)
    memory = []
    for line in log.splitlines():
        if not line.startswith("memory: "):
            continue
        fields = dict(field.split("=", 1) for field in line[len("memory: "):].split())
        phase = fields.pop("phase")
        memory.append((phase, {name: int(value) for name, value in fields.items()}))
    return measured, memory


def load(results):
    """Each case of `CASES` run in `results`, parsed, by its name."""
    cases = {}
    for case in CASES:
        directory = results / f"linux-checkout-{case}" / f"linux-checkout-{case}"
        if directory.is_dir():
            stdout = (directory / "stdout.txt").read_text()
            cases[case] = parse(stdout, (directory / "log.txt").read_text())
    return cases


def seconds(value):
    """A time in milliseconds, as seconds or milliseconds, whichever reads better."""
    return f"{value / 1000:.2f} s" if value >= 1000 else f"{value:.1f} ms"


def time_of(measured, role, name):
    value = measured.get(role, {}).get(name)
    return "" if value is None else seconds(value[0])


def spread(values, show):
    """`values` shown by `show`, as one or as their least and most."""
    if not values:
        return ""
    low, high = min(values), max(values)
    return show(low) if low == high else f"{show(low)} - {show(high)}"


def table(header, rows):
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    lines += ["| " + " | ".join(row) + " |" for row in rows]
    return "\n".join(lines)


def timings(cases, roles):
    """The timings of `roles` of each case, a column each."""
    columns = [(case, role) for role in roles for case in cases]
    header = [""] + [f"{case} {role}" for case, role in columns]
    rows = []
    for name in CHECKOUT:
        row = [time_of(cases[case][0], role, name) for case, role in columns]
        if any(row):
            rows.append([name] + row)
    return table(header, rows)


def sandboxes(cases):
    """The shared volume's timings, a column for each case over a base: the sandboxes' alone,
    their least and most; then with hundreds mounted, and those closed after a reboot."""
    shared = [case for case in cases if not case.startswith("ext4")]
    rows = []
    for name in SANDBOX:
        row = []
        for case in shared:
            measured = cases[case][0]
            values = [named[name][0] for role, named in sorted(measured.items())
                      if role.startswith("sandbox-") and name in named]
            row.append(spread(values, seconds))
        rows.append([f"sandbox {name}"] + row)
    for role, names in (("hundreds", HUNDREDS), ("rebooted", REBOOTED)):
        for name in names:
            rows.append([f"{role} {name}"] + [time_of(cases[case][0], role, name)
                                             for case in shared])
    return table([""] + shared, rows)


def facts(cases):
    """The checkout and its volume: sizes and counts, a column each case."""
    names = [("checkout", "tracked-files"), ("checkout", "volume-files"),
             ("checkout", "volume-directories"), ("checkout", "volume-bytes"),
             ("checkout", "git-bytes"), ("checkout", "read-bytes"), ("checkout", "manifest"),
             ("host", "volume-layers"), ("host", "volume-allocator-records")]
    rows = []
    for role, name in names:
        row = []
        for case in cases:
            value = cases[case][0].get(role, {}).get(name)
            row.append("" if value is None else f"{int(value[0]):,} {value[1]}")
        if any(row):
            rows.append([name] + row)
    return table([""] + list(cases), rows)


def memory(case, lines):
    """Each memory phase of `case`: the guest's memory in use (MemTotal less MemAvailable), the VM
    process's resident memory, of it its anonymous memory (the guest's RAM it touched) and its
    shared memory (the daemon's pages its window maps), the rest its files' pages (a pmem device's
    image's), the daemon's own share, and its arenas' bytes, in MiB."""
    rows = []
    for phase, fields in lines:
        if "vmm_anon" not in fields:
            continue
        guest = fields["guest_total"] - fields["guest_available"]
        rows.append([phase] + [f"{value / MIB:,.0f}" for value in (
            guest, fields["vmm_rss"], fields["vmm_anon"], fields["vmm_shmem"], fields["extentd_pss"],
            fields.get("extentd_memory_bytes", 0))])
    header = [f"{case}: phase", "guest used", "VM RSS", "VM anon", "VM shmem", "extentd PSS",
              "extentd arenas"]
    return table(header, rows)


def disk(case, measured):
    """What each store, bucket and the part cache take of the host's disk after each VM, in
    MiB."""
    host = measured.get("host", {})
    roles, entries = [], []
    for name in host:
        role, _, entry = name.partition("-disk-")
        if entry and DISK.match(entry):
            roles += [role] if role not in roles else []
            entries += [entry] if entry not in entries else []
    rows = []
    for role in roles:
        row = []
        for entry in entries:
            value = host.get(f"{role}-disk-{entry}")
            row.append("" if value is None else f"{value[0] / MIB:,.1f}")
        rows.append([role] + row)
    return table([f"{case}: after"] + entries, rows)


def report(cases):
    parts = ["## The checkout", facts(cases),
             "## Checking it out, and opening it in a fresh VM", timings(cases, ["checkout",
                                                                                "reopened"])]
    if any(not case.startswith("ext4") for case in cases):
        parts += ["## Shared, a volume over the published base", sandboxes(cases)]
    parts.append("## Memory, MiB")
    parts += [memory(case, lines) for case, (_, lines) in cases.items()]
    parts.append("## The host's disk, MiB")
    parts += [disk(case, measured) for case, (measured, _) in cases.items()]
    return "\n\n".join(parts) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("results", type=Path, help="scripts/test-e2e.sh's results directory")
    args = parser.parse_args()
    cases = load(args.results)
    if not cases:
        print(f"no linux-checkout case in {args.results}", file=sys.stderr)
        return 1
    sys.stdout.write(report(cases))
    return 0


if __name__ == "__main__":
    sys.exit(main())
