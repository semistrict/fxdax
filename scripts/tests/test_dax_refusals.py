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
FREED = (PREFIX + '(4 so far): ExtentId { branch: 1, sequence: 5 } from 0x0, 0x1000 bytes from '
         '0x21b000, read-only; not open in the window; in the store: Ok(None); '
         'the page\'s latest MAPs and UNMAPs: [\n')
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
REFUSED = ("2026-11-01T18:20:09.277538431 [anonymous-instance:dax faults] fxdax: the DAX window "
           "refused {what}, at offset 0x308000, by thread 128 ({thread}): Err(Errno(EACCES))\n")
WORKER = REFUSED.format(what="a store to a page the window maps read-only",
                        thread="kworker/1:2+events")
VCPU = REFUSED.format(what="a store to a page the window maps read-only", thread="fc_vcpu 1")
LOAD = REFUSED.format(what="an access to nothing mapped", thread="fc_vcpu 1")
# As a jailed VMM logs them, with no /proc to name a thread by: whose by its registry (TASK-167).
JAILED_WORKER = REFUSED.format(what="a store to a page the window maps read-only",
                               thread="gone, not a vCPU")
JAILED_VCPU = REFUSED.format(what="a store to a page the window maps read-only",
                             thread="gone, a vCPU")


class DaxRefusals(unittest.TestCase):
    def test_seals_explain_stores_to_extents_freed_since(self):
        self.assertEqual(dax_refusals.unexplained([FREED]), [])

    def test_seals_explain_stores_to_sealed_extents_read_only(self):
        self.assertEqual(dax_refusals.unexplained([SEALED, HISTORY, SEALED]), [])

    def test_stores_to_open_extents_are_unexplained(self):
        self.assertEqual(dax_refusals.unexplained([SEALED, OPEN, HISTORY]), [OPEN])

    def test_stores_to_pages_mapped_writably_are_unexplained(self):
        self.assertEqual(dax_refusals.unexplained([WRITABLE]), [WRITABLE])

    def test_what_a_refusal_met_later_is_no_refusal(self):
        self.assertEqual(dax_refusals.unexplained([MET]), [])

    def test_stores_of_kernel_workers_are_no_guests(self):
        self.assertEqual(dax_refusals.by_guests([WORKER]), [])

    def test_stores_of_vcpus_are_guests(self):
        self.assertEqual(dax_refusals.by_guests([WORKER, VCPU, LOAD]), [VCPU])

    def test_the_vmm_says_whose_a_store_was(self):
        """TASK-167: a jailed VMM's thread is gone to /proc, its registry saying whose it was."""
        self.assertEqual(dax_refusals.by_guests([JAILED_WORKER, JAILED_VCPU]), [JAILED_VCPU])


if __name__ == "__main__":
    unittest.main()
