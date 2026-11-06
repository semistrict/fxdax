# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/pgbench-probes.py, what a pgbench run says of DAX's write path."""

import unittest

import script

pgbench_probes = script.load("pgbench-probes")

COUNTERS = (
    "window_maps 100 window_maps_ns 4000000 window_map_requests 10 "
    "dax_writes_ns 3000000 dax_writes 10"
)


class Probes(unittest.TestCase):
    def test_means_each_probe(self):
        means = pgbench_probes.probes(pgbench_probes.stats(f"x\nfxfs stats: {COUNTERS}\n"))
        self.assertEqual(means, [("window MAP", 40.0, 80), ("write(2)", 300.0, 560)])

    def test_a_map_is_measured_alone_whatever_its_request_holds(self):
        """TASK-164.6: since TASK-164.10, a MAP request holds ~38 maps (MAP_MANY), each ~53 us,
        a request ~2 ms: a request's mean measured how many maps a request held."""
        batched = "window_maps 3800 window_maps_ns 201400000 window_map_requests 100"
        means = pgbench_probes.probes(
            pgbench_probes.stats(f"fxfs stats: {batched} dax_writes_ns 1 dax_writes 1")
        )
        self.assertEqual(means[0], ("window MAP", 53.0, 80))

    def test_takes_the_last_line(self):
        console = f"fxfs stats: dax_writes 1\nfxfs stats: {COUNTERS}\n"
        self.assertEqual(pgbench_probes.stats(console)["dax_writes"], "10")

    def test_no_stats_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "no `fxfs stats:` line"):
            pgbench_probes.stats("nothing here")

    def test_nothing_measured_is_an_error(self):
        counters = pgbench_probes.stats(f"fxfs stats: {COUNTERS.replace('dax_writes 10', 'dax_writes 0')}")
        with self.assertRaisesRegex(ValueError, "no dax_writes: nothing measured"):
            pgbench_probes.probes(counters)
