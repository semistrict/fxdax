# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of .github/workflows/ci.yml's test job: each part installs what its tests run, and no part
what only another's does. Every part fetching Coccinelle and its OCaml, three quarters of the
downloads, a slow mirror took 40 minutes of the workspace part's 90, which its tests then outran
(run 42089880804, TASK-172)."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CI = ROOT / ".github" / "workflows" / "ci.yml"
# The packages only some parts' tests run, and the parts: Coccinelle's spatch, which the scripts'
# tests run (test_cocci_guest.py); mkfs.xfs, which the guest tests' own do.
OWN = {"coccinelle": {"clippy scripts"}, "xfsprogs": {"guest"}}


def job():
    """The test job's text."""
    text = CI.read_text()
    return re.search(r"^  test:\n(.*?)^  \S", text, re.MULTILINE | re.DOTALL).group(1)


def parts():
    """The parts the job's matrix runs."""
    listed = re.search(r"^        parts: \[(.*)\]$", job(), re.MULTILINE).group(1)
    return [part.strip() for part in listed.split(",")]


def packages():
    """Each part's packages: those every part installs, and its own (matrix.include's)."""
    install = re.search(r"sudo apt-get install -y ((?:.*\\\n)*.*)$", job(), re.MULTILINE)
    common = set(install.group(1).replace("\\", " ").split()) - {"${{", "matrix.packages", "}}"}
    own = dict(re.findall(r"^          - parts: (.+)\n            packages: (.+)$", job(),
                          re.MULTILINE))
    return {part: common | set(own.get(part, "").split()) for part in parts()}


class Ci(unittest.TestCase):
    def test_each_part_installs_its_own_tools_alone(self):
        installed = packages()
        self.assertEqual(sorted(installed), sorted(["workspace", "sim", "shuttle",
                                                    "clippy scripts", "guest"]))
        for package, users in OWN.items():
            self.assertEqual({part for part, has in installed.items() if package in has}, users,
                             package)

    def test_every_part_installs_what_the_builds_need(self):
        for part, has in packages().items():
            self.assertLessEqual({"clang", "elfutils", "sqlite3", "libseccomp-dev"}, has, part)

    def test_no_build_needs_protoc_or_libclang(self):
        """What lets the job install neither: the gRPC API compiled by protox, not protoc, and no
        bindgen, which loads libclang, in either workspace. One that comes needs them back."""
        self.assertIn("protox::compile", (ROOT / "crates/fxdax/build.rs").read_text())
        for lock in [ROOT / "Cargo.lock", ROOT / "guest/tests/Cargo.lock"]:
            self.assertNotIn('name = "bindgen"', lock.read_text(), lock)
