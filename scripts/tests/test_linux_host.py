# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/linux-host-setup.sh, the library of the scripts that run on a host, here
with a temporary directory as the host's work directory; and of the tree the Lima VM's scripts
run in, which scripts/lib/lima.sh syncs with it, here with a temporary directory as the VM's home,
where a limactl in its place runs the VM's shell."""

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

from script import SCRIPTS

LIBRARY = SCRIPTS / "lib" / "linux-host-setup.sh"
# When a run's checkout was made, and when an earlier run built the host's tree: later.
CHECKED_OUT = 978307200
BUILT = 1420070400
# What the tree built, which stays, and what the checkout's did, which is not copied.
BUILDS = ["target/debug/x", "guest/tests/target/debug/x", "vendor/firecracker/build/x"]
# The VM's shell: `limactl shell INSTANCE -- bash -lc SCRIPT` runs SCRIPT here, its home $FAKE_VM,
# in an environment of its own, as Lima's.
LIMACTL = """#!/bin/bash
[[ $1 == shell && $3 == -- && $4 == bash ]] || { echo "limactl: no such call: $*" >&2; exit 2; }
shift 4
exec env -i HOME="$FAKE_VM" PATH=/usr/bin:/bin bash "$@"
"""
# A script the VM runs, which says what it was given.
SAYS = '#!/bin/bash\necho "$0 $*, LIBCLANG_PATH=${LIBCLANG_PATH:-}"\n'


def write(path, text, mtime):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.utime(path, (mtime, mtime))


def files(directory):
    return sorted(str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file())


def built(tree, source):
    """`tree`, as an earlier run built it, and `source`, a checkout made before that run: a file
    changed since, of the same size, so that only its content tells; one unchanged; one removed;
    what each built; Git's; and a directory named build that is source."""
    write(source / "src" / "changed.rs", "new\n", CHECKED_OUT)
    write(tree / "src" / "changed.rs", "old\n", BUILT)
    write(source / "src" / "same.rs", "same\n", CHECKED_OUT)
    write(tree / "src" / "same.rs", "same\n", BUILT)
    write(tree / "src" / "gone.rs", "gone\n", BUILT)
    for build in BUILDS:
        write(tree / build, "built here\n", BUILT)
        write(source / build, "built there\n", CHECKED_OUT)
    write(source / ".git" / "HEAD", "ref: refs/heads/main\n", CHECKED_OUT)
    write(source / "crates" / "c" / "build" / "build.rs", "fn main() {}\n", CHECKED_OUT)


class Synced:
    def assertSynced(self, tree, start):
        """That `tree`, as built() has it, now has the checkout's content, written where it
        differed, and its own builds; the scripts aside, which the checkout may have."""
        self.assertEqual(
            [file for file in files(tree) if not file.startswith("scripts/")],
            [
                ".by-content",
                "crates/c/build/build.rs",
                "guest/tests/target/debug/x",
                "src/changed.rs",
                "src/same.rs",
                "target/debug/x",
                "vendor/firecracker/build/x",
            ],
        )
        self.assertEqual((tree / "src" / "changed.rs").read_text(), "new\n")
        # Newer than the build, which cargo then rebuilds.
        self.assertGreaterEqual(int((tree / "src" / "changed.rs").stat().st_mtime), start)
        self.assertEqual((tree / "src" / "same.rs").stat().st_mtime, BUILT)
        self.assertEqual([(tree / build).read_text() for build in BUILDS], ["built here\n"] * 3)


class Tree(Synced, unittest.TestCase):
    def host_tree(self, work, source):
        result = subprocess.run(
            [
                "/bin/bash",
                "-c",
                'set -euo pipefail; source "$1"; work=$2 repo=$3; host_tree t; printf %s "$repo"',
                "bash",
                LIBRARY,
                work,
                source,
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual((result.returncode, result.stderr, result.stdout), (0, "", str(work / "t")))

    def test_the_tree_is_written_where_it_differs_and_keeps_its_builds(self):
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        # Synced by content before.
        (work / "t" / ".by-content").touch()
        start = int(time.time())
        self.host_tree(work, source)
        self.assertSynced(work / "t", start)

    def test_a_tree_synced_by_times_is_written_whole_once(self):
        # Synced by times, as trees were (rsync -a): its content may be newer than its times say,
        # beside a build made from what was there before.
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        start = int(time.time())
        self.host_tree(work, source)
        self.assertGreaterEqual(int((work / "t" / "src" / "same.rs").stat().st_mtime), start)
        os.utime(work / "t" / "src" / "same.rs", (BUILT, BUILT))
        self.host_tree(work, source)
        self.assertEqual((work / "t" / "src" / "same.rs").stat().st_mtime, BUILT)

    def test_a_tree_copied_by_times_since_is_written_whole_again(self):
        # Copied by content, then by times, as the scripts before did (rsync -a --delete): the
        # changed file has the checkout's time again, older than the tree's build, and the content
        # the copy by content wrote.
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(work / "t", source)
        self.host_tree(work, source)
        subprocess.run(
            ["rsync", "-a", "--delete", "--exclude", "target", "--exclude", ".git",
             f"{source}/", f"{work / 't'}/"],
            check=True,
        )
        self.assertEqual((work / "t" / "src" / "changed.rs").stat().st_mtime, CHECKED_OUT)
        start = int(time.time())
        self.host_tree(work, source)
        self.assertGreaterEqual(int((work / "t" / "src" / "changed.rs").stat().st_mtime), start)


class Lima(Synced, unittest.TestCase):
    """The Lima VM's tree, ~/fxdax-test, which scripts/lib/lima.sh syncs before it runs a script
    there, from a checkout of its own."""

    def lima(self, script, *args):
        """Runs the checkout's scripts/`script` with `args`, the checkout and the VM's tree as
        built() has them, the tree synced by content before: its status, output and errors."""
        self.vm, checkout = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        built(self.vm / "fxdax-test", checkout)
        (self.vm / "fxdax-test" / ".by-content").touch()
        for name in ["lib/lima.sh", "lib/linux-host-setup.sh", "test-linux.sh"]:
            (checkout / "scripts" / name).parent.mkdir(parents=True, exist_ok=True)
            (checkout / "scripts" / name).symlink_to(SCRIPTS / name)
        for name in ["bench.sh", "test.sh"]:
            (checkout / "scripts" / name).write_text(SAYS)
            (checkout / "scripts" / name).chmod(0o755)
        bin_ = Path(tempfile.mkdtemp())
        (bin_ / "limactl").write_text(LIMACTL)
        (bin_ / "limactl").chmod(0o755)
        self.start = int(time.time())
        done = subprocess.run(
            ["/bin/bash", checkout / "scripts" / script, *args],
            env={
                "PATH": f"{bin_}:/usr/bin:/bin",
                "HOME": tempfile.mkdtemp(),
                "FAKE_VM": str(self.vm),
                "LIBCLANG_PATH": "/usr/lib/llvm 19/lib",
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout, done.stderr

    def test_a_script_runs_in_the_vms_tree_synced_by_content(self):
        ran = self.lima("lib/lima.sh", "bench.sh", "3", "a b")
        script = self.vm / "fxdax-test" / "scripts" / "bench.sh"
        self.assertEqual(ran, (0, f"{script} 3 a b, LIBCLANG_PATH=/usr/lib/llvm 19/lib\n", ""))
        self.assertSynced(self.vm / "fxdax-test", self.start)

    def test_the_tests_run_in_the_vms_tree_synced_by_content(self):
        ran = self.lima("test-linux.sh", "-E", "test(x)")
        script = self.vm / "fxdax-test" / "scripts" / "test.sh"
        self.assertEqual(ran, (0, f"{script} -E test(x), LIBCLANG_PATH=/usr/lib/llvm 19/lib\n", ""))
        self.assertSynced(self.vm / "fxdax-test", self.start)


if __name__ == "__main__":
    unittest.main()
