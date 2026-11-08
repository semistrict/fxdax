# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/firecracker-cargo.sh, which builds Firecracker as fxdax builds it (TASK-269): in
a copy of vendor/firecracker whose vmm manifest has fxdax's crate as a dependency and whose lock is
fxdax's, neither of which the series has. In a repository of its own laid out as this one, with a
cargo that records how it was run."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

VMM_MANIFEST = '[package]\nname = "vmm"\n\n[features]\nfxdax = []\n\n[dependencies]\nlibc = "0.2"\n'


class FirecrackerCargoTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve() / "fxdax"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy(SCRIPTS / "firecracker-cargo.sh", self.root / "scripts")
        self.src = self.root / "vendor/firecracker"
        (self.src / "src/vmm").mkdir(parents=True)
        (self.src / "src/vmm/Cargo.toml").write_text(VMM_MANIFEST)
        (self.src / "Cargo.lock").write_text("the series' lock\n")
        (self.src / "README.md").write_text("upstream\n")
        (self.src / ".git").write_text("gitdir: elsewhere\n")
        (self.root / "vendor/firecracker.lock").write_text("fxdax's lock\n")
        self.tree = self.root / "target/firecracker"
        # cargo: each run's directory and words in calls; `metadata --locked` fails if $STALE.
        bin_ = Path(scratch.name) / "bin"
        bin_.mkdir()
        self.calls = Path(scratch.name) / "calls"
        (bin_ / "cargo").write_text(
            '#!/bin/bash\necho "$PWD $*" >> "$CALLS"\n'
            '[[ -z $STALE || "$*" != "metadata --locked"* ]]\n')
        (bin_ / "cargo").chmod(0o755)
        self.env = {**os.environ, "PATH": f"{bin_}:{os.environ['PATH']}",
                    "CALLS": str(self.calls)}
        for name in ["FXDAX_FIRECRACKER_SRC", "FXDAX_FIRECRACKER_TREE",
                     "FXDAX_FIRECRACKER_UNLOCKED"]:
            self.env.pop(name, None)

    def run_script(self, *args, **env):
        return subprocess.run([self.root / "scripts/firecracker-cargo.sh", *args],
                              capture_output=True, text=True, env={**self.env, **env})

    def cargo_calls(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []

    def test_builds_in_a_copy_with_fxdaxs_crate_and_lock(self):
        done = self.run_script("build", "-p", "firecracker", "--features", "fxdax")
        self.assertEqual((done.returncode, done.stderr), (0, ""))
        self.assertEqual(
            (self.tree / "src/vmm/Cargo.toml").read_text(),
            VMM_MANIFEST + "\n# fxdax: its crate, which the fxdax feature builds against "
            "(scripts/firecracker-cargo.sh).\n"
            f'[dependencies.fxdax]\npath = "{self.root}/crates/fxdax"\n')
        self.assertEqual((self.tree / "Cargo.lock").read_text(), "fxdax's lock\n")
        self.assertEqual((self.tree / "README.md").read_text(), "upstream\n")
        self.assertFalse((self.tree / ".git").exists())
        # The series' own stay as they are.
        self.assertEqual((self.src / "src/vmm/Cargo.toml").read_text(), VMM_MANIFEST)
        self.assertEqual((self.src / "Cargo.lock").read_text(), "the series' lock\n")
        self.assertEqual(self.cargo_calls(), [
            f"{self.tree} metadata --locked --format-version 1",
            f"{self.tree} build -p firecracker --features fxdax",
        ])

    def test_a_copy_again_rewrites_only_what_changed(self):
        self.assertEqual(self.run_script("build").returncode, 0)
        manifest = self.tree / "src/vmm/Cargo.toml"
        os.utime(manifest, (1, 1))
        os.utime(self.tree / "README.md", (1, 1))
        (self.src / "README.md").write_text("changed\n")
        self.assertEqual(self.run_script("build").returncode, 0)
        self.assertEqual(manifest.stat().st_mtime, 1)
        self.assertEqual((self.tree / "README.md").read_text(), "changed\n")
        self.assertNotEqual((self.tree / "README.md").stat().st_mtime, 1)

    def test_a_stale_lock_fails_unless_unlocked(self):
        done = self.run_script("build", STALE="1")
        self.assertEqual(done.returncode, 1)
        self.assertIn("vendor/firecracker.lock is not as Cargo resolves the tree", done.stderr)
        self.assertEqual(self.cargo_calls(), [f"{self.tree} metadata --locked --format-version 1"])
        self.calls.unlink()
        done = self.run_script("build", STALE="1", FXDAX_FIRECRACKER_UNLOCKED="1")
        self.assertEqual(done.returncode, 0)
        self.assertEqual(self.cargo_calls(), [f"{self.tree} build"])

    def test_lock_writes_the_copys_lock_back(self):
        self.assertEqual(self.run_script("--lock").returncode, 0)
        self.assertEqual(self.cargo_calls(), [f"{self.tree} metadata --format-version 1"])
        (self.tree / "Cargo.lock").write_text("resolved\n")
        # A cargo that resolves the copy's lock anew, as `cargo metadata` does.
        (self.root / "resolved").write_text("resolved\n")
        self.calls.unlink()
        bin_cargo = Path(self.env["PATH"].split(":")[0]) / "cargo"
        bin_cargo.write_text('#!/bin/bash\necho "$PWD $*" >> "$CALLS"\necho resolved > Cargo.lock\n')
        self.assertEqual(self.run_script("--lock").returncode, 0)
        self.assertEqual((self.root / "vendor/firecracker.lock").read_text(), "resolved\n")

    def test_a_copy_outside_the_repo_is_refused(self):
        done = self.run_script("build", FXDAX_FIRECRACKER_TREE="/tmp/elsewhere")
        self.assertEqual(done.returncode, 2)
        self.assertIn("is not in", done.stderr)
        self.assertEqual(self.cargo_calls(), [])

    def test_tree_prints_the_copy(self):
        done = self.run_script("--tree")
        self.assertEqual((done.returncode, done.stdout), (0, f"{self.tree}\n"))


if __name__ == "__main__":
    unittest.main()
