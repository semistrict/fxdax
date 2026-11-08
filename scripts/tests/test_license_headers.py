# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/license-headers.py: each source file carries the header its origin needs, hand
mutants' patches aside, with or without the Fuchsia checkout, as CI has none."""

import unittest
from unittest import mock

from script import SCRIPTS, load

headers = load("license-headers")

FXDAX_BSD = (
    "// Copyright 2026 The fxdax Authors. All rights reserved.\n"
    "// Use of this source code is governed by a BSD-style license that can be\n"
    "// found in the LICENSE file.\n\nfn main() {}\n"
)
DUAL = (
    "// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause\n"
    "// Copyright 2026 The fxdax Authors.\n\nint main(void) { return 0; }\n"
)
# A port of Fuchsia's, citing what it ports.
PORT = (
    "// Copyright 2021 The Fuchsia Authors. All rights reserved.\n"
    "// Use of this source code is governed by a BSD-style license that can be\n"
    "// found in the LICENSE file.\n"
    "// Ported from `src/storage/fxfs/src/lib.rs`.\n\nfn main() {}\n"
)


class LicenseHeadersTest(unittest.TestCase):
    def test_mutants_patches_are_not_checked(self):
        for path in (
            "crates/fxdax/mutants/tenants-disks-unlimited.patch",
            "guest/mutants/task-149.33-chunks-only-aligned.patch",
        ):
            self.assertTrue(headers.MUTANT.search(path), path)
        for path in ("guest/linux/patches/0001-dax.patch", "crates/fxdax/src/mutants.rs"):
            self.assertFalse(headers.MUTANT.search(path), path)
        self.assertEqual([path for path in headers.files() if headers.MUTANT.search(path)], [])
        self.assertIn("crates/fxdax/mutants.sh", list(headers.files()))

    def test_the_sim_diffs_shim_is_guest_sims_license(self):
        shim = "crates/fxfs-sim-diff/shim/shim.c"
        self.assertEqual(headers.needs(shim, DUAL), ("spdx", "GPL-2.0 OR BSD-3-Clause"))
        self.assertEqual(headers.problems(shim, DUAL), [])
        self.assertEqual(
            headers.problems(shim, FXDAX_BSD.replace("//", "/*", 1)),
            ["no SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause"],
        )
        # The crate's Rust is fxdax's, BSD.
        self.assertEqual(headers.problems("crates/fxfs-sim-diff/src/lib.rs", FXDAX_BSD), [])

    def test_ports_without_the_checkout_need_fuchsias_copyright_and_one_notice(self):
        path = "crates/fxfs/src/lib.rs"
        with mock.patch.object(headers, "checkout", return_value=False):
            self.assertEqual(headers.needs(path, PORT), ("fuchsia", None))
            self.assertEqual(headers.problems(path, PORT), [])
            self.assertEqual(
                headers.problems(path, PORT.replace("The Fuchsia Authors", "Someone")),
                ["cites Fuchsia, without The Fuchsia Authors' copyright"],
            )
            self.assertEqual(
                headers.problems(path, PORT.replace("BSD-style", "some")),
                ["not one license notice, but 0"],
            )

    def test_the_tree_carries_its_headers(self):
        problems = {
            path: found
            for path in headers.files()
            if (found := headers.problems(path, (headers.ROOT / path).read_text(errors="replace")))
        }
        self.assertEqual(problems, {})

    def test_the_scripts_part_runs_it(self):
        self.assertIn('"$root/scripts/license-headers.py"', (SCRIPTS / "test.sh").read_text())


if __name__ == "__main__":
    unittest.main()
