# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/guest-sim-scope.py: guest/sim's CI jobs run whole when what they test changed
since the base, a smoke run otherwise; in a repository of its own."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS, load

SCRIPT = SCRIPTS / "guest-sim-scope.py"
scope = load("guest-sim-scope")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True,
                          text=True).stdout.strip()


class ScopeTest(unittest.TestCase):
    def test_what_guest_sim_tests_runs_it_whole(self):
        for path in (
            "guest/linux/fs/fxfs/dax.c",
            "guest/linux/patches/0010-fs-dax-insert-entry.patch",
            "guest/sim/kernel/mm.c",
            "guest/sim/zloop.sh",
            "scripts/build-guest-kernel.sh",
        ):
            self.assertEqual(scope.scope(["README.md", path]), "full", path)

    def test_anything_else_is_a_smoke_run(self):
        paths = [
            "crates/fxfs/src/lib.rs",
            "guest/linux/fs/ext4/inode.c",
            "guest/linux/fs/fxfsx/a.c",
            "guest/simulator.md",
            "scripts/build-guest-kernel.sh.orig",
            "guest/mutants/task-149.33-chunks-only-aligned.patch",
            ".github/workflows/ci.yml",
        ]
        self.assertEqual(scope.scope(paths), "smoke")
        self.assertEqual(scope.scope([]), "smoke")


class BaseTest(unittest.TestCase):
    """Against a base: a push's before, a dispatch's merge-base with origin/main, one gone."""

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.repo = Path(scratch.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.com")
        self.base = self.commit("README.md")
        # origin/main, as a fetch leaves it, at the base.
        git(self.repo, "update-ref", "refs/remotes/origin/main", self.base)

    def commit(self, path):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(f"{path} {len(git(self.repo, 'log', '--oneline', '--all').split())}\n")
        git(self.repo, "add", path)
        git(self.repo, "commit", "-q", "-m", path)
        return git(self.repo, "rev-parse", "HEAD")

    def run_scope(self, *args):
        env = {key: value for key, value in os.environ.items() if key != "FXDAX_GUEST_SIM"}
        done = subprocess.run([SCRIPT, *args], cwd=self.repo, capture_output=True, text=True,
                              env=env, check=True)
        return done.stdout.strip()

    def test_a_change_of_what_it_tests_since_the_base_is_full(self):
        self.commit("crates/a.rs")
        self.assertEqual(self.run_scope(self.base, "HEAD"), "smoke")
        self.commit("guest/sim/kernel/mm.c")
        self.assertEqual(self.run_scope(self.base, "HEAD"), "full")

    def test_no_base_is_the_merge_base_with_origin_main(self):
        self.commit("crates/a.rs")
        self.assertEqual(self.run_scope(), "smoke")
        self.assertEqual(self.run_scope("0" * 40, "HEAD"), "smoke")
        self.commit("guest/linux/fs/fxfs/dax.c")
        self.assertEqual(self.run_scope(""), "full")

    def test_a_dispatch_may_ask_for_either(self):
        self.commit("guest/sim/zloop.sh")
        for asked in ["full", "smoke"]:
            env = {**os.environ, "FXDAX_GUEST_SIM": asked}
            done = subprocess.run([SCRIPT, self.base], cwd=self.repo, capture_output=True,
                                  text=True, env=env, check=True)
            self.assertEqual(done.stdout.strip(), asked)
        env = {**os.environ, "FXDAX_GUEST_SIM": "auto"}
        done = subprocess.run([SCRIPT, self.base], cwd=self.repo, capture_output=True, text=True,
                              env=env, check=True)
        self.assertEqual(done.stdout.strip(), "full")

    def test_a_base_not_in_the_history_is_full(self):
        self.commit("crates/a.rs")
        self.assertEqual(self.run_scope("1234567890" * 4, "HEAD"), "full")


if __name__ == "__main__":
    unittest.main()
