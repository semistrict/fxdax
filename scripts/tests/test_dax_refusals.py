# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/dax-refusals.py, which finds the refused guest stores no seal explains."""

import unittest

import script

dax_refusals = script.load("dax-refusals")

PREFIX = "WARN fxdax::extentd::server: extentd: a store at DAX window offset 0x10fab000 refused "
SEALED = (PREFIX + '(1 so far): ExtentId { branch: 1, sequence: 2 } from 0x0, 0x10000 bytes from '
          '0x10fa3000, read-only; not open in the window; in the store: Ok(Some("0xa000 bytes, '
          'sealed")); the page\'s latest MAPs and UNMAPs: [\n')
OPEN = (PREFIX + '(2 so far): ExtentId { branch: 1, sequence: 3 } from 0x0, 0x1000 bytes from '
        '0x21b000, read-only; not open in the window; in the store: Ok(Some("0x1000 bytes, open")); '
        'the page\'s latest MAPs and UNMAPs: [\n')
WRITABLE = (PREFIX + '(3 so far): ExtentId { branch: 1, sequence: 4 } from 0x0, 0x1000 bytes from '
            '0x21b000, writable; open in the window; in the store: Ok(Some("0x1000 bytes, open")); '
            'the page\'s latest MAPs and UNMAPs: [\n')
HISTORY = '    "MAP of ExtentId { branch: 1, sequence: 2 } from 0x0, writably, Ok(()), 0x10000 bytes",\n'
MET = ("WARN fxdax::extents: extentd: a store refused at DAX window offset 0x1f7000, 2.7ms ago, "
       'mapping ExtentId { branch: 1, sequence: 5 } from 0x0, read-only; in the store: '
       'Ok(Some("0x1000 bytes, open")), met: UNMAP, 0x1000 bytes from 0x1f7000\n')


class DaxRefusals(unittest.TestCase):
    def test_seals_explain_stores_to_sealed_extents_read_only(self):
        self.assertEqual(dax_refusals.unexplained([SEALED, HISTORY, SEALED]), [])

    def test_stores_to_open_extents_are_unexplained(self):
        self.assertEqual(dax_refusals.unexplained([SEALED, OPEN, HISTORY]), [OPEN])

    def test_stores_to_pages_mapped_writably_are_unexplained(self):
        self.assertEqual(dax_refusals.unexplained([WRITABLE]), [WRITABLE])

    def test_what_a_refusal_met_later_is_no_refusal(self):
        self.assertEqual(dax_refusals.unexplained([MET]), [])


if __name__ == "__main__":
    unittest.main()
