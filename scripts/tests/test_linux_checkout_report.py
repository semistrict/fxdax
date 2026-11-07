# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/linux-checkout-report.py, which makes tables of what the linux-checkout cases
measured (TASK-136.1)."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import script

report = script.load("linux-checkout-report")

MIB = 1 << 20

EXT4_STDOUT = """\
measure checkout mount 58.1 ms
measure checkout clone 12737.6 ms
measure checkout status-cold 364.5 ms
measure checkout tracked-files 91166 files
OK checkout
measure host checkout-disk-ext4.img 2301857792 bytes
measure host checkout-disk-rootfs 657641472 bytes
measure reopened status-cold 684.9 ms
OK reopened
"""

FXFS_STDOUT = """\
measure checkout clone 11790.0 ms
measure checkout status-cold 696.3 ms
measure checkout manifest 600 bytes
measure host volume-allocator-records 1557 records
measure sandbox-0 mount 1.5 ms
measure sandbox-0 first-overwrite 2.9 ms
measure sandbox-1 mount 1.8 ms
measure sandbox-1 first-overwrite 2.5 ms
measure hundreds mount-median 0.4 ms
measure host sandbox-0-disk-bucket 1703936 bytes
measure host sandbox-0-disk-log.txt 16384 bytes
"""


def memory_line(phase, guest_used, anon, shmem=0):
    total = 4096 * MIB
    return (f"memory: vm=1 phase={phase} host_used=1 vmm_rss={anon} vmm_pss={anon} "
            f"vmm_anon={anon} vmm_shmem={shmem} extentd_pss={8 * MIB} "
            f"extentd_memory_bytes={MIB} guest_total={total} "
            f"guest_available={total - guest_used} guest_faults=3")


class Parse(unittest.TestCase):
    def test_measurements_by_role_and_memory_by_phase(self):
        log = "\n".join([
            f"memory: vm=0 phase=before host_used=5 extentd_pss={4 * MIB}",
            "some other line of the log",
            memory_line("checkout-booted-pinned", 190 * MIB, 233 * MIB),
        ])
        measured, memory = report.parse(EXT4_STDOUT, log)
        self.assertEqual(measured["checkout"]["clone"], (12737.6, "ms"))
        self.assertEqual(measured["checkout"]["tracked-files"], (91166.0, "files"))
        self.assertEqual(measured["reopened"], {"status-cold": (684.9, "ms")})
        self.assertEqual(measured["host"]["checkout-disk-ext4.img"], (2301857792.0, "bytes"))
        self.assertEqual([phase for phase, _ in memory], ["before", "checkout-booted-pinned"])
        self.assertEqual(memory[1][1]["vmm_anon"], 233 * MIB)
        self.assertEqual(memory[1][1]["vm"], 1)


class Report(unittest.TestCase):
    def cases(self):
        log = memory_line("checkout-checked-out", 453 * MIB, 2915 * MIB)
        dax_log = memory_line("checkout-read", 434 * MIB, 2915 * MIB, 451 * MIB)
        return {"ext4": report.parse(EXT4_STDOUT, log),
                "fxfs-dax": report.parse(FXFS_STDOUT, dax_log)}

    def test_timings_compare_the_filesystems(self):
        self.assertEqual(report.timings(self.cases(), ["checkout", "reopened"]), "\n".join([
            "|  | ext4 checkout | fxfs-dax checkout | ext4 reopened | fxfs-dax reopened |",
            "|---|---|---|---|---|",
            "| mount | 58.1 ms |  |  |  |",
            "| clone | 12.74 s | 11.79 s |  |  |",
            "| status-cold | 364.5 ms | 696.3 ms | 684.9 ms |  |",
        ]))

    def test_sandboxes_spread_from_least_to_most(self):
        table = report.sandboxes(self.cases()).splitlines()
        self.assertEqual(table[0], "|  | fxfs-dax |")
        self.assertIn("| sandbox mount | 1.5 ms - 1.8 ms |", table)
        self.assertIn("| sandbox first-overwrite | 2.5 ms - 2.9 ms |", table)
        self.assertIn("| sandbox import |  |", table)
        self.assertIn("| hundreds mount-median | 0.4 ms |", table)

    def test_memory_is_in_mib(self):
        _, lines = self.cases()["fxfs-dax"]
        self.assertEqual(report.memory("fxfs-dax", lines), "\n".join([
            "| fxfs-dax: phase | guest used | VM anon | VM shmem | extentd PSS | extentd arenas |",
            "|---|---|---|---|---|---|",
            "| checkout-read | 434 | 2,915 | 451 | 8 | 1 |",
        ]))

    def test_disk_leaves_out_logs_and_the_root(self):
        measured, _ = self.cases()["ext4"]
        self.assertEqual(report.disk("ext4", measured), "\n".join([
            "| ext4: after | ext4.img |",
            "|---|---|",
            "| checkout | 2,195.2 |",
        ]))

    def test_facts_name_the_volume(self):
        self.assertEqual(report.facts(self.cases()), "\n".join([
            "|  | ext4 | fxfs-dax |",
            "|---|---|---|",
            "| tracked-files | 91,166 files |  |",
            "| manifest |  | 600 bytes |",
            "| volume-allocator-records |  | 1,557 records |",
        ]))


class Main(unittest.TestCase):
    def test_reads_a_results_directory(self):
        with tempfile.TemporaryDirectory() as results:
            case = Path(results) / "linux-checkout-ext4" / "linux-checkout-ext4"
            case.mkdir(parents=True)
            (case / "stdout.txt").write_text(EXT4_STDOUT)
            (case / "log.txt").write_text(memory_line("checkout-read", 340 * MIB, 2925 * MIB))
            done = subprocess.run([sys.executable, str(script.SCRIPTS / "linux-checkout-report.py"),
                                   results], capture_output=True, text=True, check=True)
        self.assertTrue(done.stdout.startswith("## The checkout\n"))
        self.assertIn("| checkout-read | 340 | 2,925 | 0 | 8 | 1 |", done.stdout)
        self.assertNotIn("## Shared", done.stdout)

    def test_says_when_there_is_nothing(self):
        with tempfile.TemporaryDirectory() as results:
            done = subprocess.run([sys.executable, str(script.SCRIPTS / "linux-checkout-report.py"),
                                   results], capture_output=True, text=True)
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stderr, f"no linux-checkout case in {results}\n")


if __name__ == "__main__":
    unittest.main()
