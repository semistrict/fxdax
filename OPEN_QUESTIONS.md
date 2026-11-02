# Open questions

## Sign in to GCE

gcloud's credentials expired. A Google sign-in page opened in your browser on 2026-11-02: finish it as ramon@echophase.com.
**Until then:** no GCE runs: the `load_during_move` hang can't be repeated, and nothing new gets a real-kernel check.

What the agents need from you, and nothing else. See AGENTS.md. Answer by editing an entry, or say so in any session.

## How to close truncate on a full disk? (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Either every non-DAX growth takes one path that zeroes the tail, or a mid-block shrink may return `NoSpace`, partly reversing TASK-60.
**Recommend:** the first, with one owner of file size (TASK-153.4).
**Until then:** the default model test could very rarely go red.

## What should catch a corrupted byte of file data? (TASK-153.11)

The problem: a byte of a file gets damaged in storage, by a bad disk or a bad upload. Should a read fail with an error, or return the wrong byte?

What protects data today:
- **Sealed data on the host and in the bucket has a checksum:** `extent_frame` stores a CRC32C for each part and checks it when reading. I haven't checked that every read path goes through it.
- **Fxfs's own data checksums are never read,** except at journal replay, where they show whether a write reached the device. That's how Fuchsia's code has it. In a test that flips bytes in an Fxfs image on a fake device, about 84% read back wrong with no error. That test skips the host layer, so it overstates the risk on a real host. Reading them wouldn't help DAX anyway: DAX stores skip Fxfs's write path, so they have no checksum.
- **Once a page is in the DAX window, a read goes straight to memory,** so nothing can check it there.

Not covered, as far as I know: data on a host's disk before it's sealed, and pages already in memory.

Options: rely on the host's checksums and close the gaps there; or have the guest check too, which costs reads on the fast path.
**Recommend:** rely on the host. First confirm every host read path checks its CRC, which needs no decision from you, so I'm doing that anyway.
**Until then:** nothing changes.

## What should a fork hold of an extent mapped writably? (TASK-153.3)

A fork while an extent is mapped writably now gets `EBUSY` (TASK-150.7). Finishing "open only once" on the host means choosing what the fork holds instead: the extent as of the last commit, or the mapped bytes too. Production forks use `fork_quiescent`, so this reaches only a VM process's own raw FORK.
**Recommend:** as of the last commit, which is what the spec says.
**Until then:** `EBUSY`, and the host half of TASK-153.3 waits.

## A subagent deleted a test without asking

`specs/dax-region/mutations/NoOpenUnderLock.cfg`, because the rule it tests went away with TASK-153.3; `NoOneWay` is said to catch the same bug. It's on `refactor/open-once`, not main. I've told it to put the file back unless it can't pass.
**Recommend:** keep it deleted only if it truly can't pass, as you'll see in that task's notes.
**Until then:** it isn't merged.

## Push `fix/main-after-push`?

Two test fixes and this file. `scripts/tests` is red on main until they land, because two mutants went stale. No product code changes.
**Recommend:** push.
**Until then:** this file isn't on main, and main's test run reports a failure.
