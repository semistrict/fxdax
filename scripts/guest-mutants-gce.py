#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Hand mutants of the guest's DAX code (guest/linux/fs/fxfs/dax.c), run on a GCE test host
(TASK-44.19).

cargo-mutants changes Rust alone; the guest's C has its locks, and its checks of signals and pins,
which a mutant here takes out, one at a time (writable DAX's, of its gate, seals and moves, went
with it, 2026-11-04): in a worktree of HEAD, the mutant's edits made,
scripts/test-gce.sh runs the DAX suites on a guest kernel the host builds from it. The host keeps
its kernel's build directory, so each mutant recompiles fs/fxfs alone. A mutant the suites fail
on is killed; one they pass, missed, needs a test, or is recorded here as equivalent, with why.

    scripts/guest-mutants-gce.py HOST ZONE RESULTS [MUTANT...]
    scripts/guest-mutants-gce.py --check [MUTANT...]

runs every mutant (or those named) on HOST, made with scripts/test-gce.sh create, writing each
run's results under RESULTS, and prints each mutant's fate. --check runs nothing: it makes each
mutant's edits of the sources as they are, in memory, as CI does, and fails if one isn't found
once, which would have the mutant test nothing. FXDAX_E2E_TEST selects the suites, by name
or by tag, as scripts/test-e2e.sh's arguments do, the mutants suite by default
(`runner list --tag mutants`); FXDAX_MUTANT_KERNEL the kernel the host builds for every mutant (debug, with
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
DATA = "guest/linux/fs/fxfs/data.c"

# Each mutant: its name, what it takes out, and its edits, (old, new) text of dax.c, or (old, new,
# path) of another file, each of which must be found there once; and, for one only an assertion of a debug kernel's finds, the
# kernel the host builds (FXDAX_GUEST_KERNEL_BUILD: debug, with lockdep), release otherwise.
MUTANTS = [
    # Missed by every suite here (2026-11-02); killed by guest/sim's reaches_at_once, which held a
    # MAP at the host as a second one, further, ended (TASK-44.19), and went with writable DAX.
    (
        "map_to_unlocked",
        "a region's MAPs and its mapped go without the region's lock: two MAP as far as each "
        "found mapped, and the one that ends last says so (TASK-44.17)",
        [("\tint err = 0;\n\n\tmutex_lock(&region->lock);\n", "\tint err = 0;\n\n"),
         ("out:\n\tmutex_unlock(&region->lock);\n\treturn err;\n}", "out:\n\treturn err;\n}")],
    ),
    (
        "resize_moves_the_last_block",
        "a DAX file's change of size has Fxfs zero its last block past the end by moving it, from "
        "under the window, which goes on mapping it (TASK-150.9)",
        [("\terr = fxfs_vfs_write_attr(inode, FXFS_SET_SIZE | (ops->keeps_tail ? FXFS_SET_KEEP_TAIL : 0));",
          "\terr = fxfs_vfs_write_attr(inode, FXFS_SET_SIZE);", DATA)],
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
]

# Equivalent mutants, which no suite can kill, and why:

SUITES = "--tag mutants"


class Unfound(Exception):
    """A mutant's edit not found once in its file."""


def mutated(tree, edits):
    """The files of `tree` a mutant's edits change, as they make them, each in turn."""
    texts = {}
    for old, new, *file in edits:
        path = tree / (file[0] if file else DAX)
        text = texts.get(path) or path.read_text()
        if text.count(old) != 1:
            raise Unfound(f"{path}: a mutant's edit is found {text.count(old)} times: {old!r}")
        texts[path] = text.replace(old, new)
    return texts


def mutate(tree, edits):
    try:
        for path, text in mutated(tree, edits).items():
            path.write_text(text)
    except Unfound as unfound:
        raise SystemExit(str(unfound))


def check(names):
    """Each mutant's edits found once in the sources as they are: a line for each that isn't."""
    broken = []
    for name, _, edits, *_ in MUTANTS:
        if names and name not in names:
            continue
        try:
            mutated(ROOT, edits)
        except Unfound as unfound:
            broken.append(f"{name}: {unfound}")
    return broken


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
    if sys.argv[1:2] == ["--check"]:
        broken = check(set(sys.argv[2:]))
        for line in broken:
            print(line, file=sys.stderr)
        print(f"mutants: {len(MUTANTS)} checked, {len(broken)} broken")
        raise SystemExit(1 if broken else 0)
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
