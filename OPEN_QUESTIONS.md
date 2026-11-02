# Open questions

## Push `fix/main-after-push`?

Two test fixes, this file, AGENTS.md's rule to decide rather than wait, a guest watchdog that dumps every task's stack before a test's timeout, and the fix of `load_during_move`'s hang, which was the check deadlocking itself (r528: 20 rounds pass). `scripts/tests` is red on main until the first two land, because two mutants went stale. No product code changes.
**Recommend:** push.
**Until then:** none of this is on main, main's test run reports a failure, and `fxfs-guest-checks-dax*` can hang on `load_during_move`.

## Merge and push `refactor/open-once` (TASK-153.3)?

Extents open only once, guest and host: main (106b6128d) merged in, the host half done on the fork decision below, and a stale read of 106b6128d's fixed (157417ad5). Workspace, sim, TLC and GCE r477 pass; pgbench 968 tps against main's 960. The one WARN left, `fs/dax.c:416`, is main's too (TASK-150.21). Head 68f897b02, a `wip:` commit whose message wants its final wording first.
**Recommend:** merge into main and push, after this branch.
**Until then:** main keeps the double open of extents and the fork's `EBUSY`.

# Decisions

## Truncate on a full disk: one path zeroes the tail (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Decided: every non-DAX growth takes one path that zeroes the tail, with one owner of file size (TASK-153.4).
**Instead:** a mid-block shrink could return `NoSpace`, partly reversing TASK-60.
**Undoing it:** before TASK-153.4 lands, nothing; after, the single path stays and only the shrink's error changes.

## Corrupted file data: the host's checksums catch it (TASK-153.11)

A byte damaged in storage, by a bad disk or a bad upload, must make a read fail rather than return the wrong byte. Decided: rely on the host's CRC32C in `extent_frame`, confirm every host read path checks it, and close gaps there. The guest doesn't check.
- Fxfs's own data checksums are read only at journal replay, as in Fuchsia's code, and DAX stores skip Fxfs's write path so have none.
- Pages already in the DAX window are read straight from memory, so nothing can check them there.
- Still uncovered: data on a host's disk before it's sealed, and pages already in memory.

**Instead:** the guest checks too, costing reads on the fast path.
**Undoing it:** adding guest checks later doesn't conflict with anything this does.

## A fork of an extent mapped writably holds it as of the last commit (TASK-153.3)

Today such a fork gets `EBUSY` (TASK-150.7). Decided: the fork holds the extent as of the last commit, as the spec says, so the host half of TASK-153.3 goes ahead. This reaches only a VM process's own raw FORK: production forks use `fork_quiescent`.
**Instead:** the fork also holds the mapped, uncommitted bytes.
**Undoing it:** the fork path in TASK-153.3 copies the mapped bytes as well.
