# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/lib/linux-host-setup.sh, the library of the scripts that run on a host, here
with a temporary directory as the host's work directory."""

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


def write(path, text, mtime):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.utime(path, (mtime, mtime))


def files(directory):
    return sorted(str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file())


class Tree(unittest.TestCase):
    def test_the_tree_is_written_where_it_differs_and_keeps_its_builds(self):
        work, source = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp())
        tree = work / "t"
        # Changed since the tree was built, in a checkout older than that build: of the same size,
        # so that only its content tells.
        write(source / "src" / "changed.rs", "new\n", CHECKED_OUT)
        write(tree / "src" / "changed.rs", "old\n", BUILT)
        write(source / "src" / "same.rs", "same\n", CHECKED_OUT)
        write(tree / "src" / "same.rs", "same\n", BUILT)
        write(tree / "src" / "gone.rs", "gone\n", BUILT)
        # What the tree built, which stays, and what the checkout's did, which is not copied.
        builds = ["target/debug/x", "guest/tests/target/debug/x", "vendor/firecracker/build/x"]
        for build in builds:
            write(tree / build, "built here\n", BUILT)
            write(source / build, "built there\n", CHECKED_OUT)
        # A directory named so elsewhere is source.
        write(source / "crates" / "c" / "build" / "build.rs", "fn main() {}\n", CHECKED_OUT)
        start = int(time.time())
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
        self.assertEqual((result.returncode, result.stderr, result.stdout), (0, "", str(tree)))
        self.assertEqual(
            files(tree),
            [
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
        self.assertEqual([(tree / build).read_text() for build in builds], ["built here\n"] * 3)


if __name__ == "__main__":
    unittest.main()
