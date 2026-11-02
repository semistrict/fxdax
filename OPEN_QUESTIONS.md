# Open questions

What the agents need from you, and nothing else. See AGENTS.md. Answer by editing an entry, or say so in any session.

## Push the batch?

`fix/refusal-gate` is ahead of main with: `generic/129`'s fix, transactions reporting their room, cross-host tenant restores (your VM manager must do the steps in `docs/integrator-guide.md`), the corrupt image fixes, the fault limit, and three flaky test fixes. Main fails two tests intermittently until it lands.
**Recommend:** push once the batch's GCE run passes.
**Until then:** it stays local. Agents don't push without your word.

## Must one snapshot restore on several hosts at once? (TASK-158)

Today, once one host forks a point, every other host's copy fails with `EIO` until it's dropped and created again. Fixing that needs a protocol change.
**Recommend:** yes if you fan out from one base; no otherwise.
**Until then:** one host at a time works; `docs/integrator-guide.md` says so.

## How to close truncate on a full disk? (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Either every non-DAX growth takes one path that zeroes the tail, or a mid-block shrink may return `NoSpace`, partly reversing TASK-60.
**Recommend:** the first, with one owner of file size (TASK-153.4).
**Until then:** the default model test could very rarely go red.

## What should catch a corrupted byte of file data? (TASK-153.11)

The problem: a byte of a file gets damaged in storage, by a bad disk or a bad upload. Should a read fail with an error, or return the wrong byte?

What protects data today:
- **Sealed data on the host and in the bucket has a checksum:** `extent_frame` stores a CRC32C for each part and checks it when reading. I haven't checked that every read path goes through it.
- **Fxfs's own data checksums are never read,** except at journal replay. In a test that flips bytes in an Fxfs image on a fake device, about 84% read back wrong with no error. That test skips the host layer, so it overstates the risk on a real host.
- **Once a page is in the DAX window, a read goes straight to memory,** so nothing can check it there.

Not covered, as far as I know: data on a host's disk before it's sealed, and pages already in memory.

Options: rely on the host's checksums and close the gaps there; or have the guest check too, which costs reads on the fast path.
**Recommend:** rely on the host. First confirm every host read path checks its CRC, which needs no decision from you, so I'm doing that anyway.
**Until then:** nothing changes.
