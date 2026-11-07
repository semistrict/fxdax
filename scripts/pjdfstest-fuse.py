#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""pjdfstest (BSD-2-Clause) on FUSE mounts of new stores, as root (TASK-149.38): POSIX's
permissions, owners, setgid and sticky directories, times and errnos, as the host's Node layer,
fxfs-posix under it and fxdax::fuse's server over it keep them.

Two runs, each on a new store fxdax-mount makes (`--new`) and mounts:

- in a directory made in the mount, as the guest's pjdfstest case runs in its Fxfs;
- from the mount's root itself, chowned and chmodded first: a new volume's root has no POSIX
  attributes of its own, and a chown of it alone once left it mode 000 (TASK-222).

pjdfstest is built at the commit scripts/lib/pjdfstest.sh pins, in FXDAX_PJDFSTEST_WORK (default:
target/pjdfstest), once. A run's failures are its failed tests, by file and number, and its files
that failed whole (a bad plan, an exit status); scripts/pjdfstest-fuse-known lists those of open
bugs, each with its task. The script fails if any other fails, or if one listed fails in neither
run, so that the list stays true. The runs' logs are left in the work directory.

Usage: sudo scripts/pjdfstest-fuse.py FXDAX_MOUNT [DIR...]

FXDAX_MOUNT is the fxdax-mount binary; DIRs name pjdfstest's directories of tests (such as chmod),
all of them otherwise.
"""

import os
import re
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIN = ROOT / "scripts" / "lib" / "pjdfstest.sh"
KNOWN = ROOT / "scripts" / "pjdfstest-fuse-known"
# The stores' size: room for what pjdfstest makes, which is little.
CAPACITY = 1 << 30


def pin():
    """The repository and commit scripts/lib/pjdfstest.sh pins, unless the environment says."""
    text = PIN.read_text()
    found = {name: re.search(rf"{name}=\$\{{{name}:-([^}}]+)\}}", text).group(1)
             for name in ("PJDFSTEST_REPO", "PJDFSTEST_COMMIT")}
    return tuple(os.environ.get(name, value) for name, value in found.items())


def build(work):
    """pjdfstest built at the pin in `work`, unless it is already: its directory."""
    repo, commit = pin()
    source = work / "pjdfstest"
    built = work / "pjdfstest.commit"
    if built.exists() and built.read_text() == commit and (source / "pjdfstest").exists():
        return source
    work.mkdir(parents=True, exist_ok=True)
    subprocess.run(["rm", "-rf", str(source)], check=True)
    run = lambda *args: subprocess.run(args, cwd=source, check=True)
    source.mkdir()
    run("git", "init", "-q")
    run("git", "fetch", "-q", "--depth", "1", repo, commit)
    run("git", "checkout", "-q", "FETCH_HEAD")
    run("autoreconf", "-ifs")
    run("./configure")
    run("make", "pjdfstest")
    built.write_text(commit)
    return source


def failures(prove):
    """The failures prove's summary says: `FILE:N` for each test failed, `FILE` for a file that
    failed whole, FILE relative to pjdfstest's tests directory."""
    found = set()
    lines = prove.splitlines()
    try:
        start = lines.index("Test Summary Report")
    except ValueError:
        return found
    current, listing = None, False
    for line in lines[start + 2:]:
        header = re.match(r"^\S*/tests/(\S+\.t)\s+\(Wstat:", line)
        if header:
            current, listing = header.group(1), False
            continue
        if current is None or not line.startswith(" "):
            current = None
            continue
        label = re.match(r"^\s+([A-Za-z -]+):\s*(.*)$", line)
        if label:
            listing = label.group(1) == "Failed tests"
            if listing:
                found |= numbers(current, label.group(2))
            elif label.group(1) in ("Parse errors", "Non-zero exit status", "Non-zero wait status"):
                found.add(current)
        elif listing:
            # The failed tests' list goes on.
            found |= numbers(current, line)
    return found


def numbers(file, listed):
    """`FILE:N` of each test a list of numbers and ranges such as `3, 7-9` names."""
    found = set()
    for part in filter(None, (part.strip() for part in listed.split(","))):
        first, _, last = part.partition("-")
        for number in range(int(first), int(last or first) + 1):
            found.add(f"{file}:{number}")
    return found


def known(text):
    """The failures scripts/pjdfstest-fuse-known lists, each with its task: `TASK-N FAILURE`, a
    FAILURE `FILE:N` or `FILE`, lines starting with `#` comments."""
    listed = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        task, failure = line.split()[:2]
        if not re.fullmatch(r"TASK-[0-9.]+", task):
            raise ValueError(f"{line!r}: not a task first")
        listed[failure] = task
    return listed


def unexpected(found, listed, dirs):
    """The failures `found` that `listed` doesn't list, and those listed, of the directories run,
    that weren't found: (unlisted, gone)."""
    covered = lambda failure: failure in listed or failure.split(":")[0] in listed
    unlisted = sorted(failure for failure in found if not covered(failure))
    ran = lambda failure: not dirs or failure.split("/")[0] in dirs
    gone = sorted(failure for failure in listed if ran(failure)
                  and not any(seen == failure or seen.split(":")[0] == failure for seen in found))
    return unlisted, gone


def mounted(fxdax_mount, dir):
    """fxdax-mount of a new store in `dir`, once it says it is mounted: the process, the mount
    point and its log."""
    store, mountpoint = dir / "store", dir / "mnt"
    mountpoint.mkdir()
    log = open(dir / "fxdax-mount.log", "w")
    # Other users' too, which pjdfstest's tests become (`-u`, `-g`): a FUSE mount is its mounting
    # user's alone otherwise.
    command = [fxdax_mount, "--allow-other", "--new", str(CAPACITY), str(store), str(mountpoint)]
    process = subprocess.Popen(command,
                               stdout=subprocess.PIPE, stderr=log, text=True)
    said = process.stdout.readline().strip()
    if said != str(mountpoint):
        process.wait()
        sys.exit(f"fxdax-mount: not mounted, {process.returncode}: "
                 f"{(dir / 'fxdax-mount.log').read_text()}")
    return process, mountpoint, log


def run(name, fxdax_mount, tests, dirs, setup, logs):
    """One run: a new store mounted, `setup` given its mount point and saying where prove runs,
    then pjdfstest; the mount unmounted. Its failures."""
    with tempfile.TemporaryDirectory(prefix=f"pjdfstest-fuse-{name}-") as dir:
        # Searchable by the users pjdfstest's tests become, as the mount is.
        os.chmod(dir, 0o755)
        process, mountpoint, log = mounted(fxdax_mount, Path(dir))
        try:
            cwd = setup(mountpoint)
            paths = [str(tests / d) for d in dirs] or [str(tests)]
            proved = subprocess.run(["prove", "-r", *paths], cwd=cwd, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, text=True)
        finally:
            process.send_signal(signal.SIGTERM)
            status = process.wait()
            log.close()
        (logs / f"{name}.log").write_text(proved.stdout)
        print(f"== {name}: prove exit {proved.returncode}; {logs / f'{name}.log'}")
        print("\n".join(proved.stdout.splitlines()[-4:]))
        if status != 0:
            sys.exit(f"fxdax-mount exited {status}: {(Path(dir) / 'fxdax-mount.log').read_text()}")
        found = failures(proved.stdout)
        if proved.returncode != 0 and not found:
            sys.exit(f"prove failed, {proved.returncode}, and said no test failed")
        return found


def in_a_directory(mountpoint):
    work = mountpoint / "work"
    work.mkdir()
    return work


def from_the_root(mountpoint):
    """The root chowned to another user and back, its mode kept by each chown, then chmodded."""
    for owner in (65534, 0):
        os.chown(mountpoint, owner, owner)
        mode = os.stat(mountpoint).st_mode & 0o7777
        if mode != 0o755:
            sys.exit(f"a chown of the mount's root to {owner} left it mode {mode:o}")
    os.chmod(mountpoint, 0o755)
    return mountpoint


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__.split("Usage: ")[1].split("\n")[0])
    if os.geteuid() != 0:
        sys.exit("pjdfstest-fuse.py runs as root: pjdfstest chowns, and mounts by mount(2)")
    fxdax_mount, dirs = os.path.abspath(sys.argv[1]), sys.argv[2:]
    work = Path(os.environ.get("FXDAX_PJDFSTEST_WORK", ROOT / "target" / "pjdfstest")).resolve()
    tests = build(work) / "tests"
    found = set()
    for name, setup in (("in-a-directory", in_a_directory), ("from-the-root", from_the_root)):
        found |= run(name, fxdax_mount, tests, dirs, setup, work)
    listed = known(KNOWN.read_text())
    unlisted, gone = unexpected(found, listed, dirs)
    for failure in sorted(found - set(unlisted)):
        print(f"known: {failure} ({listed.get(failure) or listed[failure.split(':')[0]]})")
    for failure in unlisted:
        print(f"FAILED: {failure}")
    for failure in gone:
        print(f"PASSED, listed as failing ({listed[failure]}): {failure}: take it out of {KNOWN}")
    sys.exit(1 if unlisted or gone else 0)


if __name__ == "__main__":
    main()
