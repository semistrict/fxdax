# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/firecracker-forward.sh, which refuses a vendor/firecracker moving back or
sideways (TASK-166): in a repository of its own whose commits name the fork's commits, against a
gh that answers GitHub's compare as the test says."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

SCRIPT = SCRIPTS / "firecracker-forward.sh"
OLD, NEW = "a" * 40, "b" * 40


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True,
                          text=True).stdout.strip()


class FirecrackerForwardTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)
        self.repo = self.dir / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.com")
        # gh: the compare's status from $COMPARE, each call's words in calls.
        bin_ = self.dir / "bin"
        bin_.mkdir()
        (bin_ / "gh").write_text(
            '#!/bin/bash\necho "$@" >> "$CALLS"\necho "$COMPARE"\n')
        (bin_ / "gh").chmod(0o755)
        self.env = {**os.environ, "PATH": f"{bin_}:{os.environ['PATH']}",
                    "CALLS": str(self.dir / "calls")}

    def commit(self, firecracker):
        """A commit whose vendor/firecracker is the fork's commit `firecracker`."""
        git(self.repo, "update-index", "--add", "--cacheinfo",
            f"160000,{firecracker},vendor/firecracker")
        git(self.repo, "commit", "-q", "-m", firecracker[:4])
        return git(self.repo, "rev-parse", "HEAD")

    def check(self, old, new, compare):
        done = subprocess.run([SCRIPT, old, new], cwd=self.repo, capture_output=True, text=True,
                              env={**self.env, "COMPARE": compare})
        return done.returncode, done.stderr

    def calls(self):
        path = self.dir / "calls"
        return path.read_text().splitlines() if path.exists() else []

    def test_a_move_forward_passes(self):
        old, new = self.commit(OLD), self.commit(NEW)
        self.assertEqual(self.check(old, new, "ahead"), (0, ""))
        self.assertEqual(
            self.calls(),
            [f"api repos/semistrict/firecracker/compare/{OLD}...{NEW} --jq .status"])

    def test_a_move_back_fails(self):
        """b781ff302's: main's Firecracker back to before F_SEAL."""
        old, new = self.commit(NEW), self.commit(OLD)
        self.assertEqual(
            self.check(old, new, "behind"),
            (1, f"vendor/firecracker goes from {NEW[:12]} to {OLD[:12]}, behind of it: not "
                "forward\n"))

    def test_a_move_sideways_fails(self):
        old, new = self.commit(OLD), self.commit(NEW)
        self.assertEqual(self.check(old, new, "diverged")[0], 1)

    def test_no_move_asks_nothing(self):
        old = self.commit(OLD)
        git(self.repo, "commit", "-q", "--allow-empty", "-m", "same")
        self.assertEqual(self.check(old, "HEAD", "behind"), (0, ""))
        self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
