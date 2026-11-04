# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/pgbench-gate.py, which gates a GCE run of pgbench, and of copy-on-write DAX's
cases, on their throughput and memory."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import script

gate = script.load("pgbench-gate")

THRESHOLDS = {"decay": 0.9, "dax": 0.95, "dax-cow": 0.95, "forked-dax": 0.0,
              "forked-dax-cow": 0.9, "static-dax": 0.0, "density": 0.9, "density-memory": 0.8,
              "density-memory-mib": 768.0, "bigmap-read": 1.0, "bigmap-guest-cache": 0.05,
              "bigmap-written-memory": 2.0}
MIB = 1 << 20


def least(name, case, value, threshold, passed):
    """Gate `name` of `case`, whose `value` is to be at least its `threshold`, as it `passed`."""
    return {"gate": name, "case": case, "value": value, "threshold": threshold,
            "bound": "at least", "passed": passed}


def most(name, case, value, threshold, passed):
    """Gate `name` of `case`, whose `value` is to be at most its `threshold`, as it `passed`."""
    return {"gate": name, "case": case, "value": value, "threshold": threshold,
            "bound": "at most", "passed": passed}


def log(*vms):
    """A case's log of `vms`, each its runs' tps, as a run keeps it: its console (log.txt), where
    pgbench says each run's tps, and its guest's output (stdout.txt), where the guest says how many
    runs each VM made."""
    console = ["pgbench: at 1.15 s: initdb -D /mnt/fxfs/pgdata -A trust"]
    stdout = ["OK base"]
    for runs in vms:
        for tps in runs:
            console += ["latency average = 3.840 ms", "initial connection time = 4.1 ms",
                        f"tps = {tps} (without initial connection time)"]
        stdout += ["OK load 5000000", f"OK runs {len(runs)}", "OK checked"]
    return "\n".join(console) + "\n", "\n".join(stdout) + "\n"


def density_log(per_vm_mib, tps=(100, 80, 80)):
    """A density case's log of a VM a run of each of `tps`: its console (log.txt), where the host
    says its memory before the first VM and after each VM's runs, each VM costing it `per_vm_mib`,
    and its guest's output (stdout.txt)."""
    console = [f"memory: vm=0 phase=base host_used={1000 * MIB} extentd_pss={10 * MIB}"]
    stdout = ["OK load 5000000", "OK base"]
    for vm, each in enumerate(tps, 1):
        console += [
            "fxfs dax_cow: on",
            f"tps = {each} (without initial connection time)",
            f"memory: vm={vm} phase=ran host_used={(1000 + vm * per_vm_mib) * MIB} "
            f"vmm_pss={per_vm_mib * MIB} vmm_anon={(per_vm_mib - 50) * MIB} vmm_shmem={50 * MIB} "
            f"extentd_memory_bytes={vm * 100 * MIB} extentd_private_pages={vm * 10} "
            f"guest_total={1024 * MIB} guest_available={512 * MIB} guest_cached={256 * MIB}",
        ]
        stdout += ["OK runs 1", "OK checked"]
    return "\n".join(console) + "\n", "\n".join(stdout) + "\n"


def bigmap_log(read, cached, written_mib=200, mode="on"):
    """A bigmap case's log: its cold reads of its 4 GiB file at `read` MiB/s, over which its guest's
    page cache grows by `cached` MiB and the host's memory by 100; its 200 MiB of writes, over which
    the host's grows by `written_mib`, and which its guest counts private; and its guest's
    output."""
    console = [f"fxfs dax_cow: {mode}", f"bigmap: file_bytes {4096 * MIB}.000",
               f"bigmap: read_cold_mib_s {read}.000", "bigmap: write_pages 51200.000",
               "bigmap: store_p99_us 120.500"]
    used, guest = 1000, 0
    for phase in gate.BIGMAP_PHASES:
        if phase == "read-cold":
            used, guest = used + 100, cached
        elif phase == "written":
            used += written_mib
        privates = 51200 if phase in ("written", "synced", "end") else 0
        vm = ("vm=0" if phase == "base"
              else f"vm=1 vmm_pss={50 * MIB} guest_fxfs_window_privates={privates}")
        console.append(f"memory: {vm} phase={phase} host_used={used * MIB} "
                       f"extentd_memory_bytes={used * MIB // 2} guest_cached={guest * MIB}")
    return "\n".join(console) + "\n", "OK base\nOK checked\n"


class PgbenchGate(unittest.TestCase):
    def results(self, cases, rounds=False):
        """A results directory of `cases`, each case's log by its name: in e2e/CASE/CASE, or, if
        `rounds`, each of a list of logs in e2e/CASE/round-K/CASE."""
        root = Path(tempfile.mkdtemp())
        for case, logged in cases.items():
            for at, (console, stdout) in enumerate(logged if rounds else [logged], 1):
                place = root / "e2e" / case / (f"round-{at}" if rounds else case)
                if rounds:
                    place = place / case
                place.mkdir(parents=True)
                (place / "log.txt").write_text(console)
                (place / "stdout.txt").write_text(stdout)
        return root

    def test_runs_are_split_by_vm(self):
        console, stdout = log([1, 2, 3], [4, 5, 6])
        self.assertEqual(gate.runs((console + stdout).splitlines()),
                         [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])

    def test_vms_are_told_apart_by_the_guest_s_output_beside_the_console(self):
        console, stdout = log([1, 2], [3, 4])
        place = self.results({"fxfs-guest-pgbench-dax": (console, stdout)})
        log_txt = place / "e2e" / "fxfs-guest-pgbench-dax" / "fxfs-guest-pgbench-dax" / "log.txt"
        self.assertEqual(gate.runs(gate.said(log_txt)), [[1.0, 2.0], [3.0, 4.0]])
        log_txt.with_name("stdout.txt").unlink()
        self.assertEqual(gate.runs(gate.said(log_txt)), [[1.0, 2.0, 3.0, 4.0]])

    def test_each_rounds_runs_are_measured_in_order(self):
        results = self.results({"fxfs-guest-pgbench-dax": [log([1, 2]), log([3, 4])] * 5},
                               rounds=True)
        self.assertEqual(gate.measure(results)["dax"][::5], [[[1.0, 2.0]], [[3.0, 4.0]]])
        self.assertEqual(len(gate.measure(results)["dax"]), 10)

    def test_the_team_s_decay_and_dax_against_no_dax_fail(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench": log([839, 840, 845, 838, 842]),
            "fxfs-guest-pgbench-dax": log([922.3, 811.1, 795.0, 644.8, 669.8]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            least("decay", "no-dax", 1.0036, 0.9, True),
            least("decay", "dax", 0.7262, 0.9, False),
            least("dax", "dax", 0.9464, 0.95, False),
        ])

    def test_each_runs_latencies_are_its_guests_histograms_over_it(self):
        lines = [
            "fxfs stats: seals 0 seals_hist 0,0,0,0 device_reads 3",
            "fxfs stats: seals 10 seals_hist 0,5,4,1 device_reads 9",
            "tps = 100.0 (without initial connection time)",
            "fxfs stats: seals 10 seals_hist 0,5,4,1",
            "fxfs stats: seals 12 seals_hist 2,5,4,1",
            "tps = 90.0 (without initial connection time)",
        ]
        found, per_vm = gate.records(lines)
        self.assertEqual((found, per_vm), ([
            (100.0, {"seals": {"count": 10, "p50_us": 2.048, "p90_us": 4.096, "p99_us": None}}),
            (90.0, {"seals": {"count": 2, "p50_us": 1.024, "p90_us": 1.024, "p99_us": 1.024}}),
        ], None))
        results = self.results({"fxfs-guest-pgbench-dax": ("\n".join(lines), "OK runs 2\n")})
        self.assertEqual(gate.measure_latencies(results)["dax"][0][0][1]["seals"]["count"], 2)
        self.assertEqual(gate.measure_latencies(self.results({"fxfs-guest-pgbench": log([1])})),
                         {}, "no histograms, no latencies")

    def test_one_daemon_s_last_vm_is_gated_against_its_first(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench-dax": log([100, 100, 95], [90, 85, 80]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            least("decay", "dax", 0.95, 0.9, True),
            least("density", "dax", 0.85, 0.9, False),
        ])

    def test_dax_is_reported_against_ext4_with_dax_on_pmem_which_decays_ungated(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench-dax": log([900, 880, 860]),
            "pgbench-ext4-dax-pmem": log([2000, 1000, 1500]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            least("decay", "dax", 0.9556, 0.9, True),
            least("static-dax", "dax", 0.5867, 0.0, True),
        ])
        gated = gate.gates(measured, {**THRESHOLDS, "static-dax": 0.6})
        self.assertEqual(gated[1]["passed"], False)

    def test_density_is_of_the_first_round_of_many_vms_in_any_results(self):
        single = self.results({
            "fxfs-guest-pgbench": log([1000, 1000]),
            "fxfs-guest-pgbench-dax": log([900, 900]),
        })
        density = self.results({"fxfs-guest-pgbench-dax": log([800, 800], [700, 720], [600, 600])})
        measured = gate.measure(single, density)
        self.assertEqual(measured["dax"], [[[900.0, 900.0]],
                                           [[800.0, 800.0], [700.0, 720.0], [600.0, 600.0]]])
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            least("decay", "no-dax", 1.0, 0.9, True),
            least("decay", "dax", 1.0, 0.9, True),
            least("density", "dax", 0.75, 0.9, False),
            least("dax", "dax", 0.9, 0.95, False),
        ])

    def test_the_static_dax_threshold_is_set_from_the_environment(self):
        os.environ["FXDAX_GATE_STATIC_DAX"] = "0.7"
        try:
            self.assertEqual(gate.thresholds()["static-dax"], 0.7)
        finally:
            del os.environ["FXDAX_GATE_STATIC_DAX"]

    def test_the_command_writes_json_and_fails_on_a_gate(self):
        results = self.results({"fxfs-guest-pgbench-dax": log([100, 80])})
        out = Path(tempfile.mkdtemp()) / "gate.json"
        environment = {**os.environ, "FXDAX_GATE_DECAY": "0.75"}
        passed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", "--json", out, results],
            env=environment, capture_output=True, text=True)
        self.assertEqual((passed.returncode, passed.stderr), (0, ""))
        self.assertEqual(json.loads(out.read_text()), {
            "host": "nested",
            "tps": {"dax": [[[100.0, 80.0]]]},
            "gates": [least("decay", "dax", 0.8, 0.75, True)],
        })
        failed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", "--host", "bare-metal", results],
            capture_output=True, text=True)
        self.assertEqual((failed.returncode, failed.stderr),
                         (1, "failed: decay of dax: 0.8 under 0.9\n"))
        self.assertEqual(json.loads(failed.stdout)["host"], "bare-metal")

    def test_rounds_that_passed_are_read_from_their_whole_logs(self):
        root = Path(tempfile.mkdtemp())
        case = root / "e2e" / "fxfs-guest-pgbench-dax"
        case.mkdir(parents=True)
        for at, runs in [(1, [10, 9]), (2, [8, 7])]:
            console, stdout = log(runs)
            (case / f"round-{at}.log").write_text(console + stdout)
        self.assertEqual(gate.measure(root)["dax"], [[[10.0, 9.0]], [[8.0, 7.0]]])

    def test_a_run_without_pgbench_fails(self):
        empty = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", tempfile.mkdtemp()],
            capture_output=True, text=True)
        self.assertEqual(empty.returncode, 1)
        self.assertRegex(empty.stderr, r"^no pgbench runs in ")

    def test_copy_on_write_and_a_base_s_database_are_gated_against_no_dax(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench": log([1000, 1000, 1000]),
            "fxfs-guest-pgbench-dax-cow": log([960, 950, 940]),
            "fxfs-guest-pgbench-forked": log([800, 800, 800]),
            "fxfs-guest-pgbench-dax-forked": log([400, 420, 410]),
            "fxfs-guest-pgbench-dax-cow-forked": log([700, 720, 710]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            least("decay", "no-dax", 1.0, 0.9, True),
            least("decay", "dax-cow", 0.9792, 0.9, True),
            least("decay", "no-dax-forked", 1.0, 0.9, True),
            least("decay", "dax-forked", 1.025, 0.9, True),
            least("decay", "dax-cow-forked", 1.0143, 0.9, True),
            least("dax-cow", "dax-cow", 0.95, 0.95, True),
            least("forked-dax", "dax-forked", 0.5125, 0.0, True),
            least("forked-dax-cow", "dax-cow-forked", 0.8875, 0.9, False),
        ])

    def test_the_host_s_memory_is_read_as_said(self):
        self.assertEqual(gate.samples([
            "memory: vm=0 phase=base host_used=100 extentd_pss=7",
            "tps = 1.0 (without initial connection time)",
            "  memory: vm=1 phase=ran host_used=300 vmm_pss=150 guest_cached=20  ",
        ]), [
            {"vm": 0, "phase": "base", "host_used": 100, "extentd_pss": 7},
            {"vm": 1, "phase": "ran", "host_used": 300, "vmm_pss": 150, "guest_cached": 20},
        ])

    def test_a_density_case_s_memory_is_what_each_vm_after_the_first_adds(self):
        console, _ = density_log(300)
        self.assertEqual(gate.density_memory(gate.samples(console.splitlines())), {
            "vms": 3,
            "first_vm_mib": 300.0,
            "per_added_vm_mib": 300.0,
            "host_used_mib": [1000.0, 1300.0, 1600.0, 1900.0],
            "vmm_pss_mib": [300.0, 300.0, 300.0],
            "vmm_anon_mib": [250.0, 250.0, 250.0],
            "vmm_shmem_mib": [50.0, 50.0, 50.0],
            "extentd_memory_mib": [100.0, 200.0, 300.0],
            "guest_used_mib": [512.0, 512.0, 512.0],
            "guest_cached_mib": [256.0, 256.0, 256.0],
            "extentd_private_pages": [10, 20, 30],
        })
        one_vm = [f"memory: vm=0 phase=base host_used={MIB}",
                  f"memory: vm=1 phase=ran host_used={3 * MIB}"]
        self.assertEqual(gate.density_memory(gate.samples(one_vm))["per_added_vm_mib"], None)
        self.assertEqual(gate.density_memory(gate.samples(one_vm[1:])), None, "no VM 0, no cost")

    def test_dax_density_memory_is_gated_at_most_against_no_dax_and_in_mib(self):
        density, _ = gate.measure_memory(self.results({
            "fxfs-guest-pgbench-density": density_log(1000),
            "fxfs-guest-pgbench-dax-density": density_log(300),
            "fxfs-guest-pgbench-dax-cow-density": density_log(900),
        }))
        self.assertEqual(sorted(density), ["dax-cow-density", "dax-density", "no-dax-density"])
        found = gate.gates({}, THRESHOLDS, density, {})
        self.assertEqual(found, [
            most("density-memory", "dax-density", 0.3, 0.8, True),
            most("density-memory-mib", "dax-density", 300.0, 768.0, True),
            most("density-memory", "dax-cow-density", 0.9, 0.8, False),
            most("density-memory-mib", "dax-cow-density", 900.0, 768.0, False),
        ])
        without_no_dax = {name: density[name] for name in ["dax-density"]}
        self.assertEqual(gate.gates({}, THRESHOLDS, without_no_dax, {}), [
            most("density-memory-mib", "dax-density", 300.0, 768.0, True),
        ])

    def test_a_bigmap_case_s_steps_are_what_they_cost(self):
        console, _ = bigmap_log(400, 0)
        lines = console.splitlines()
        self.assertEqual(gate.bigmap_summary(gate.results(lines), gate.samples(lines)), {
            "file_bytes": 4096.0 * MIB,
            "read_cold_mib_s": 400.0,
            "write_pages": 51200.0,
            "store_p99_us": 120.5,
            "guest_cache_share": 0.0,
            "host_read_mib": 100.0,
            "host_written_mib": 200.0,
            "written_memory_per_byte": 1.0,
            "host_used_mib": {"mounted": 0.0, "read-cold": 100.0, "read-warm": 100.0,
                              "written": 300.0, "synced": 300.0, "end": 300.0},
            "vmm_pss_mib": {"mounted": 50.0, "read-cold": 50.0, "read-warm": 50.0,
                            "written": 50.0, "synced": 50.0, "end": 50.0},
            "extentd_memory_mib": {"base": 500.0, "mounted": 500.0, "read-cold": 550.0,
                                   "read-warm": 550.0, "written": 650.0, "synced": 650.0,
                                   "end": 650.0},
            "guest_fxfs_window_privates": {"base": None, "mounted": 0, "read-cold": 0,
                                           "read-warm": 0, "written": 51200, "synced": 51200,
                                           "end": 51200},
        })
        unfinished = [line for line in lines if "phase=end" not in line]
        self.assertEqual(gate.bigmap_summary(gate.results(unfinished), gate.samples(unfinished)),
                         None, "a step not measured, no summary")

    def test_bigmap_s_dax_reads_cache_and_writes_are_gated(self):
        _, bigmap = gate.measure_memory(self.results({
            "fxfs-guest-bigmap": bigmap_log(100, 1024, written_mib=10),
            "fxfs-guest-bigmap-dax": bigmap_log(400, 0, written_mib=0),
            "fxfs-guest-bigmap-dax-cow": bigmap_log(400, 8, written_mib=600),
        }))
        self.assertEqual(bigmap["bigmap-no-dax"]["guest_cache_share"], 0.25)
        self.assertEqual(gate.gates({}, THRESHOLDS, {}, bigmap), [
            least("bigmap-read", "bigmap-dax", 4.0, 1.0, True),
            most("bigmap-guest-cache", "bigmap-dax", 0.0, 0.05, True),
            least("bigmap-read", "bigmap-dax-cow", 4.0, 1.0, True),
            most("bigmap-guest-cache", "bigmap-dax-cow", 0.002, 0.05, True),
            most("bigmap-written-memory", "bigmap-dax-cow", 3.0, 2.0, False),
        ])

    def test_whether_guests_copied_on_write_is_as_they_all_said(self):
        self.assertEqual(gate.modes(["fxfs dax_cow: on", "x", "fxfs dax_cow: on"]), "on")
        self.assertEqual(gate.modes(["fxfs dax_cow: on", "fxfs dax_cow: off"]), "mixed")
        self.assertEqual(gate.modes(["tps = 1.0 (without initial connection time)"]), None)

    def test_a_new_threshold_is_set_from_the_environment(self):
        os.environ["FXDAX_GATE_DENSITY_MEMORY_MIB"] = "512"
        try:
            self.assertEqual(gate.thresholds()["density-memory-mib"], 512.0)
        finally:
            del os.environ["FXDAX_GATE_DENSITY_MEMORY_MIB"]

    def test_the_command_reports_memory_bigmap_and_copy_on_write_and_fails_over_a_ceiling(self):
        results = self.results({
            "fxfs-guest-pgbench-density": density_log(1000, tps=(100, 95, 95)),
            "fxfs-guest-pgbench-dax-density": density_log(300, tps=(100, 95, 95)),
            "fxfs-guest-bigmap": bigmap_log(100, 1024, written_mib=10, mode="absent"),
            "fxfs-guest-bigmap-dax-cow": bigmap_log(400, 0, written_mib=600),
        })
        out = Path(tempfile.mkdtemp()) / "gate.json"
        failed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", "--json", out, results],
            capture_output=True, text=True)
        self.assertEqual((failed.returncode, failed.stderr),
                         (1, "failed: bigmap-written-memory of bigmap-dax-cow: 3.0 over 2.0\n"))
        report = json.loads(out.read_text())
        self.assertEqual(sorted(report), ["bigmap", "dax_cow", "gates", "host", "memory",
                                          "provisional", "tps"])
        self.assertEqual(report["provisional"], ["bigmap-guest-cache", "bigmap-read",
                                                 "bigmap-written-memory", "density-memory",
                                                 "density-memory-mib"])
        self.assertEqual(report["dax_cow"], {"no-dax-density": "on", "dax-density": "on",
                                             "bigmap-no-dax": "absent", "bigmap-dax-cow": "on"})
        self.assertEqual(report["memory"]["dax-density"]["per_added_vm_mib"], 300.0)
        self.assertEqual(report["bigmap"]["bigmap-dax-cow"]["written_memory_per_byte"], 3.0)
        self.assertEqual(report["tps"]["dax-density"], [[[100.0], [95.0], [95.0]]])

    def test_bigmap_alone_is_gated(self):
        results = self.results({"fxfs-guest-bigmap-dax-cow": bigmap_log(400, 0)})
        passed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", results],
            capture_output=True, text=True)
        self.assertEqual((passed.returncode, passed.stderr), (0, ""))
        self.assertEqual(json.loads(passed.stdout)["tps"], {})
