#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Hand mutants of the guest's DAX code (guest/linux/fs/fxfs/dax.c), run on a GCE test host
(TASK-44.19).

cargo-mutants changes Rust alone; the guest's C has its locks, gate checks and seals rechecks,
which a mutant here takes out, one at a time: in a worktree of HEAD, the mutant's edits made,
scripts/test-gce.sh runs the DAX suites on a guest kernel the host builds from it. The host keeps
its kernel's build directory, so each mutant recompiles fs/fxfs alone. A mutant the suites fail
on is killed; one they pass, missed, needs a test, or is recorded here as equivalent, with why.

    scripts/guest-mutants-gce.py HOST ZONE RESULTS [MUTANT...]

runs every mutant (or those named) on HOST, made with scripts/test-gce.sh create, writing each
run's results under RESULTS, and prints each mutant's fate. FXDAX_E2E_TEST names the suites, the
DAX ones by default; the other FXDAX_* settings reach the host as scripts/test-gce.sh says.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DAX = "guest/linux/fs/fxfs/dax.c"

# Each mutant: its name, what it takes out, and its edits, (old, new) text of dax.c, each of
# which must be found there once.
MUTANTS = [
    (
        "region_open_unlocked",
        "region_open looks at mapped without the region's lock (TASK-99)",
        [("\tfxfs_dax_delay();\n\tmutex_lock(&region->lock);\n\tspin_lock(&dax->regions_lock);",
          "\tfxfs_dax_delay();\n\tspin_lock(&dax->regions_lock);"),
         ("\tspin_unlock(&dax->regions_lock);\n\tmutex_unlock(&region->lock);\n}",
          "\tspin_unlock(&dax->regions_lock);\n}")],
    ),
    (
        "map_chunk_no_seals_recheck",
        "a chunk's map takes its extents for open though a seal came between",
        [("\t\tif (gate == seals) {\n\t\t\terr = fxfs_dax_map_extents(dax, range, extents, count);\n"
          "\t\t\tfxfs_device_store_end(dax->dev);",
          "\t\tif (true) {\n\t\t\terr = fxfs_dax_map_extents(dax, range, extents, count);\n"
          "\t\t\tfxfs_device_store_end(dax->dev);")],
    ),
    (
        "new_slice_no_seals_recheck",
        "a slice's region is opened though a seal came between",
        [("\t\tif (gate == seals) {\n\t\t\tfxfs_dax_region_open(dax, region);",
          "\t\tif (true) {\n\t\t\tfxfs_dax_region_open(dax, region);")],
    ),
    (
        "write_no_seals_recheck",
        "a write maps the blocks it moved though a seal came between",
        [("\t\tif (gate == seals) {\n\t\t\terr = fxfs_dax_map_extents(dax, range, extents, count);\n"
          "\t\t\tif (err) {",
          "\t\tif (true) {\n\t\t\terr = fxfs_dax_map_extents(dax, range, extents, count);\n"
          "\t\t\tif (err) {")],
    ),
    (
        "seal_keeps_writable",
        "a seal leaves ranges' pages taken for writable",
        [("\t\t\tbitmap_zero(range->writable, range->pages);\n", "")],
    ),
    (
        "relocate_unmarked",
        "a page moved is not marked relocated, its old mappings kept",
        [("\t\t\tset_bit(page + i, range->relocated);", "\t\t\t;")],
    ),
    (
        "cow_not_shared",
        "an iomap of a relocated page doesn't say it is shared, its entry not replaced",
        [("\tiomap->flags |= IOMAP_F_SHARED;\n", "")],
    ),
    (
        "prune_in_use",
        "a range lets go of regions while another iomap uses it",
        [("\tif (!nr || atomic_read(&range->users) != 1 ||",
          "\tif (!nr ||")],
    ),
    (
        "held_not_written",
        "a fault's page held is not written into its new block, which holds zeros (TASK-79.11)",
        [("size, fault, held,\n", "size, false, NULL,\n")],
    ),
    (
        "evacuation_unlocked",
        "an evacuation maps a file's pages after letting go of its semaphore, which a write may map "
        "anew meanwhile (TASK-101)",
        [("\tfxfs_dax_evacuee_gather(file, &ev);\n\tfxfs_dax_evacuee_map(dax, file, &ev);\n",
          "\tfxfs_dax_evacuee_gather(file, &ev);\n\tup_write(&file->sem);\n"
          "\tfxfs_dax_evacuee_map(dax, file, &ev);\n\tdown_write(&file->sem);\n"),
         ("\tlockdep_assert_held_write(&file->sem);\n\tif (ev->count) {\n\t\tfxfs_dax_delay();",
          "\tif (ev->count) {\n\t\tfxfs_dax_delay();")],
    ),
]

SUITES = "fxfs-guest-dax fxfs-guest-checks-dax fxfs-guest-checks-dax-delay fxfs-guest-pgbench-dax"


def mutate(tree, edits):
    path = tree / DAX
    text = path.read_text()
    for old, new in edits:
        if text.count(old) != 1:
            raise SystemExit(f"{DAX}: a mutant's edit is found {text.count(old)} times: {old!r}")
        text = text.replace(old, new)
    path.write_text(text)


def run(host, zone, results, name, edits):
    with tempfile.TemporaryDirectory(prefix="fxdax-mutant-") as scratch:
        tree = Path(scratch) / "tree"
        subprocess.run(["git", "-C", ROOT, "worktree", "add", "-q", "--detach", tree, "HEAD"],
                       check=True)
        try:
            subprocess.run(["git", "-C", tree, "submodule", "update", "--init", "-q",
                            "vendor/firecracker"], check=True)
            mutate(tree, edits)
            env = dict(os.environ, FXDAX_GCE_ZONE=zone, FXDAX_WORKSPACE_TESTS="0",
                       FXDAX_GUEST_KERNEL_BUILD="release",
                       FXDAX_E2E_TEST=os.environ.get("FXDAX_E2E_TEST", SUITES))
            out = Path(results) / name
            log = open(Path(results) / f"{name}.log", "w")
            ran = subprocess.run([tree / "scripts/test-gce.sh", "run", host, out], env=env,
                                 stdout=log, stderr=subprocess.STDOUT, cwd=tree)
            return ran.returncode
        finally:
            subprocess.run(["git", "-C", ROOT, "worktree", "remove", "--force", tree], check=True)


def main():
    if len(sys.argv) < 4:
        raise SystemExit(__doc__)
    host, zone, results, names = sys.argv[1], sys.argv[2], sys.argv[3], set(sys.argv[4:])
    Path(results).mkdir(parents=True, exist_ok=True)
    for name, what, edits in MUTANTS:
        if names and name not in names:
            continue
        status = run(host, zone, results, name, edits)
        fate = "killed" if status != 0 else "MISSED"
        print(f"{fate} {name}: {what}", flush=True)


if __name__ == "__main__":
    main()
