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
DAX ones by default; FXDAX_MUTANT_KERNEL the kernel the host builds for every mutant (debug, with
lockdep and KCSAN, or kasan), each's own otherwise; the other FXDAX_* settings reach the host as
scripts/test-gce.sh says.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DAX = "guest/linux/fs/fxfs/dax.c"
SUPER = "guest/linux/fs/fxfs/super.c"

# Each mutant: its name, what it takes out, and its edits, (old, new) text of dax.c, or (old, new,
# path) of another file, each of which must be found there once; and, for one only an assertion of a debug kernel's finds, the
# kernel the host builds (FXDAX_GUEST_KERNEL_BUILD: debug, with lockdep), release otherwise.
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
        [("\t\tif (gate == seals) {\n\t\t\tbool fail = count > 1 &&",
          "\t\tif (true) {\n\t\t\tbool fail = count > 1 &&")],
    ),
    (
        "seal_keeps_writable",
        "a seal leaves ranges' pages taken for writable",
        [("\t\t\tbitmap_zero(range->writable, range->pages);\n", "")],
    ),
    (
        "held_not_written",
        "a fault's page held is not written into its new block, which holds zeros (TASK-79.11)",
        [("size, fault, held,\n", "size, false, NULL,\n")],
    ),
    (
        "moved_unmapped",
        "a write whose blocks moved before a seal shut the gate leaves its range mapping them where "
        "they were, whose extent Fxfs frees (TASK-121)",
        [("\tif (moved) {\n\t\tint mapped", "\tif (false) {\n\t\tint mapped")],
    ),
    (
        "map_failure_unretried",
        "a write whose pieces' MAP failed after its blocks moved fails, its range left mapping "
        "blocks Fxfs freed, rather than dropped and the write tried again (TASK-79.106)",
        [
            (
                "\trange->behind = true;\n\tunmap_mapping_pages(inode->i_mapping, "
                "fxfs_dax_chunk_start(range) >> PAGE_SHIFT, range->pages,\n\t\t\t    false);\n"
                "\treturn err == -EXDEV ? err : -ESTALE;\n",
                "\treturn err;\n",
            )
        ],
    ),
    (
        "regions_find_unlocked",
        "a region is looked up without the regions lock, which fxfs_dax_find_region asserts held: "
        "a lockdep kernel's assertion fails (TASK-44.9)",
        [("\tspin_lock(&dax->regions_lock);\n\tregion = fxfs_dax_find_region(dax, e->branch, "
          "e->sequence);\n\tif (region)\n\t\tregion->refs++;\n\tspin_unlock(&dax->regions_lock);"
          "\n\tif (region)\n\t\treturn region;\n",
          "\tregion = fxfs_dax_find_region(dax, e->branch, e->sequence);\n\tif (region)\n"
          "\t\tregion->refs++;\n\tif (region)\n\t\treturn region;\n")],
        "debug",
    ),
    (
        "inherited_acls_written_back",
        "a created inode's inherited ACLs go with write_inode, not with its creation, which an "
        "fsync of another inode makes durable without them (TASK-79.29)",
        [("\terr = fxfs_vfs_acl_xattr(default_acl, ACL_TYPE_DEFAULT, mode, xattrs, &nr_xattrs,\n"
          "\t\t\t\t &values[0]);\n\tif (!err)\n"
          "\t\terr = fxfs_vfs_acl_xattr(acl, ACL_TYPE_ACCESS, mode, xattrs, &nr_xattrs, "
          "&values[1]);\n\tif (!err)\n\t\terr = fxfs_nofs(fxfs_create(",
          "\tif (!err)\n\t\terr = fxfs_nofs(fxfs_create(", SUPER),
         ("\tif (default_acl && S_ISDIR(mode))\n"
          "\t\tset_cached_acl(d_inode(dentry), ACL_TYPE_DEFAULT, default_acl);\n\tif (acl)\n"
          "\t\tset_cached_acl(d_inode(dentry), ACL_TYPE_ACCESS, acl);\n"
          "\tposix_acl_release(default_acl);\n\tposix_acl_release(acl);\n\treturn 0;\n}\n\n"
          "/* Creates an unnamed regular file",
          "\treturn fxfs_vfs_init_acls(d_inode(dentry), default_acl, acl);\n}\n\n"
          "/* Creates an unnamed regular file", SUPER)],
    ),
    (
        "fault_ignores_fatal_signals",
        "a fault with no room looks for none of its signals: a splice holding the window can't be "
        "killed (TASK-79.49)",
        [("\t\tif (fatal_signal_pending(current)) {\n\t\t\tret = VM_FAULT_SIGBUS;\n"
          "\t\t\tbreak;\n\t\t}\n", "")],
    ),
    (
        "store_fails_on_pins",
        "a store from user mode fails on a page of a slice pinned, as a fault from the kernel does: "
        "a SIGBUS where the pin goes soon (TASK-79.49)",
        [("\tbool user = vmf->flags & FAULT_FLAG_USER;\n", "\tbool user = false;\n")],
    ),
    (
        "moved_unmapped_resealed",
        "a fault whose blocks moved, finding a seal ended since, tries again, and leaves its range "
        "mapping them where they were if the next seal shuts the gate (TASK-133)",
        [("\t\t\t\tfxfs_dax_pause(FXFS_DAX_STEP_RESEALED);\n\t\t\tcontinue;\n",
          "\t\t\t\tfxfs_dax_pause(FXFS_DAX_STEP_RESEALED);\n\t\t\tmoved = 0;\n\t\t\tcontinue;\n")],
    ),
    (
        "moved_write_not_retried",
        "a write whose chunk's map fails after Fxfs moved its blocks fails, those pages zeros "
        "(TASK-119.13)",
        [("\t\treturn fresh && (err == -EIO || err == -ENOMEM || err == -ENOSPC) ? -ESTALE : err;\n",
          "\t\treturn err;\n")],
    ),
]

# Equivalent mutants, which no suite can kill, and why:

SUITES = "fxfs-guest-dax fxfs-guest-checks-dax fxfs-guest-checks-dax-delay fxfs-guest-pgbench-dax"


def mutate(tree, edits):
    for old, new, *file in edits:
        path = tree / (file[0] if file else DAX)
        text = path.read_text()
        if text.count(old) != 1:
            raise SystemExit(f"{path}: a mutant's edit is found {text.count(old)} times: {old!r}")
        path.write_text(text.replace(old, new))


def run(host, zone, results, name, edits, build):
    with tempfile.TemporaryDirectory(prefix="fxdax-mutant-") as scratch:
        tree = Path(scratch) / "tree"
        subprocess.run(["git", "-C", ROOT, "worktree", "add", "-q", "--detach", tree, "HEAD"],
                       check=True)
        try:
            subprocess.run(["git", "-C", tree, "submodule", "update", "--init", "-q",
                            "vendor/firecracker"], check=True)
            mutate(tree, edits)
            env = dict(os.environ, FXDAX_GCE_ZONE=zone, FXDAX_WORKSPACE_TESTS="0",
                       FXDAX_GUEST_KERNEL_BUILD=build,
                       FXDAX_E2E_TEST=os.environ.get("FXDAX_E2E_TEST", SUITES))
            out = Path(results) / name
            log = open(Path(results) / f"{name}.log", "w")
            ran = subprocess.run([tree / "scripts/test-gce.sh", "run", host, out], env=env,
                                 stdout=log, stderr=subprocess.STDOUT, cwd=tree)
            # scripts/test-e2e.sh says a round failed in the guest tests' log, e2e.log.
            logs = [out / "remote.log", out / "e2e.log"]
            said = "".join(log.read_text(errors="replace") for log in logs if log.exists())
            return fate(ran.returncode, said)
        finally:
            subprocess.run(["git", "-C", ROOT, "worktree", "remove", "--force", tree], check=True)


def fate(status, logs):
    """A mutant's fate, from its run's exit status and the host's logs: killed only if a suite
    failed a round (scripts/test-e2e.sh says so, in e2e.log); a run that failed otherwise, as a
    build that doesn't compile or a host lost, is BROKEN, and says nothing of the suites
    (TASK-119.8)."""
    if status == 0:
        return "MISSED"
    return "killed" if ": failed in round " in logs else "BROKEN"


def main():
    if len(sys.argv) < 4:
        raise SystemExit(__doc__)
    host, zone, results, names = sys.argv[1], sys.argv[2], sys.argv[3], set(sys.argv[4:])
    Path(results).mkdir(parents=True, exist_ok=True)
    for name, what, edits, *build in MUTANTS:
        if names and name not in names:
            continue
        kernel = os.environ.get("FXDAX_MUTANT_KERNEL") or (build[0] if build else "release")
        ended = run(host, zone, results, name, edits, kernel)
        print(f"{ended} {name}: {what}", flush=True)


if __name__ == "__main__":
    main()
