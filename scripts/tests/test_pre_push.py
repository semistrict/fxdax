# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/hooks/pre-push, which refuses a push changing the guest's DAX code unless
the commit pushed passed scripts/dax-gate.sh, as its note says (TASK-119.8): in a repository of
its own, whose origin's main the push builds on."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

HOOK = SCRIPTS / "hooks" / "pre-push"
ZERO = "0" * 40


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True,
                          text=True).stdout.strip()


class PrePushTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.repo = Path(scratch.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.com")
        self.main = self.commit("README", "fxdax\n")
        git(self.repo, "update-ref", "refs/remotes/origin/main", self.main)

    def commit(self, path, text):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)
        git(self.repo, "add", path)
        git(self.repo, "commit", "-q", "-m", path)
        return git(self.repo, "rev-parse", "HEAD")

    def push(self, sha, remote_sha):
        """The hook, as git runs it for a push of `sha` to main, which the remote has at
        `remote_sha`: its status and what it says."""
        ran = subprocess.run([HOOK, "origin", "unused"], cwd=self.repo, capture_output=True,
                             text=True, input=f"refs/heads/main {sha} refs/heads/main {remote_sha}\n")
        return ran.returncode, ran.stderr

    def test_dax_change_without_a_pass_is_refused(self):
        sha = self.commit("guest/linux/fs/fxfs/dax.c", "changed\n")
        status, said = self.push(sha, self.main)
        self.assertEqual(status, 1)
        self.assertIn(f"{sha[:12]} has not passed scripts/dax-gate.sh", said)

    def test_driver_change_without_a_pass_is_refused(self):
        sha = self.commit("guest/linux/drivers/virtio_extent/virtio_extent.c", "changed\n")
        self.assertEqual(self.push(sha, self.main)[0], 1)

    def test_dax_change_that_passed_the_gate_is_pushed(self):
        sha = self.commit("guest/linux/fs/fxfs/dax.c", "changed\n")
        git(self.repo, "notes", "--ref=dax-gate", "add", "-m", "passed", sha)
        self.assertEqual(self.push(sha, self.main), (0, ""))

    def test_dax_change_under_a_later_commit_is_refused(self):
        self.commit("guest/linux/fs/fxfs/dax.c", "changed\n")
        sha = self.commit("README", "fxdax, again\n")
        self.assertEqual(self.push(sha, self.main)[0], 1)

    def test_other_changes_are_pushed(self):
        sha = self.commit("crates/fxdax/src/lib.rs", "changed\n")
        self.assertEqual(self.push(sha, self.main), (0, ""))

    def test_new_branch_is_compared_with_origin_main(self):
        sha = self.commit("guest/linux/fs/fxfs/dax.c", "changed\n")
        self.assertEqual(self.push(sha, ZERO)[0], 1)

    def test_deletion_is_pushed(self):
        self.assertEqual(self.push(ZERO, self.main), (0, ""))


if __name__ == "__main__":
    unittest.main()
