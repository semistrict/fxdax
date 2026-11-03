# Open questions

## The nightly needs GCP credentials in GitHub (2026-11-02)

`.github/workflows/nightly.yml` never ran: its YAML didn't parse (fixed in a1e079306). Now it parses, it needs the repository's `GCP_WORKLOAD_IDENTITY_PROVIDER` and `GCP_SERVICE_ACCOUNT` secrets and its `FXDAX_GCE_PROJECT` and `FXDAX_NIGHTLY_BUCKET` variables, none of which is set. Granting GitHub access to the GCP project is yours.
**Recommended:** a workload identity pool for `semistrict/fxdax` and a service account allowed to make and delete GCE instances and write the nightly bucket.
**Until then:** each nightly fails at its auth step; TASK-69's runs stay manual.

# Decisions

## A panic through a store fences it: fail-stop, usable again once reopened (TASK-153.16, 2026-11-03)

A panic holding a store's lock poisoned it, and every later request of its guest panicked too (TASK-104, 105, 150.20; in `extent_store`, 79.1 and 79.80). Decided: the store is fenced. Every request after the panic fails with `EIO` without panicking, the guest's filesystem stops, and so does its VM. When the connection ends, the store is closed like a writer that left writes uncommitted: it is retired and its followers are cut off. The next open sees it as of its last commit, as after a crash. `crate::fence`: each `ExtentStore` lock can only be taken through the store's fence, and a guard dropped during a panic sets it. The daemon also fences a store when a request, flush or clean of it panics below the locks.
**Instead:** (1) recover the lock's state: nothing proves it consistent across the store's 7 locks and the layers beneath them, even though the three instances happened to panic before changing anything. (2) Fence and reopen in place under the running guest: its filesystem holds the open extents a reopen would lose, and no protocol tells a writer's guest to remount, the way `ESTALE` tells a follower. (3) Turn poison into `EIO` lock by lock: a store whose `open` lock is poisoned would still map from its unpoisoned `dax` lock what the panic left half done.
**Undoing it:** for (1), `StoreMutex::lock` takes `PoisonError::into_inner` and stops checking the fence. The daemon's fencing is in `serve_in_turn`, `Answer::drop` and `flush_behind`.

## The journal's own transaction past its file stops the journal, not the kernel (TASK-60, TASK-139)

`make_room`'s asserts, that the journal's transaction extending its file fits within the old size and a chunk, now stop the journal (`Inner::terminate`) and fail the commit: the guest gets EIO and stays mounted, and replay ends before that transaction. With the default chunk the sizing holds, so the transaction wasn't rebuilt to fit the room it has.
**Instead:** size the own transaction to the room left, which would keep the filesystem writable even then.
**Undoing it:** restore the two `assert!`s in `Journal::make_room`.

## Bases fork on several hosts at once too, as points (TASK-158, 2026-11-03)

A snapshot's point now restores on any number of hosts at once. A fork of anything else, such as a base from `ImportImage`, is a commit on its branch, made by its writer, one host at a time. Decided: `ForkStoreRequest.point` (and `fxdaxctl point SRC DST`) makes a point of any store, once nothing is open, and each host forks a copy of it (`CreateStore` of its branch), as a snapshot's restores do. The base stays writable on its host; what it writes after needs a new point to reach forks. A point of a point is refused (`INVALID_ARGUMENT`). `docs/integrator-guide.md` says how.
**Instead:** bases fork on one host at a time, and the guide says so.
**Undoing it:** drop the field and `ExtentStore::point`'s use from `stores::fork`; nothing else depends on it.

## map_to_unlocked is killed by a scenario, not by seeds (TASK-44.19, 2026-11-03)

3,000 seeds of `interleave` and `failures` missed the mutant (as did every guest DAX suite): it needs two MAPs of one region in flight at once, the nearer ending last, and a seal before the grow worker maps past it again. `guest/sim`'s device now holds a MAP back when a test asks (`sim_device_hold_map`), and `tests/scenarios`' `reaches_at_once` plays the sequence out: the mutant fails it at once, `fxfs_dax_map_to` as it is passes. `zloop.sh`, CI's sim job, now runs `tests/scenarios`' scenarios too, which only `check.sh` and `mutants.sh` ran.
**Instead:** seeded MAP latency in `interleave`, so that seeds find this class without a scenario: it moves every seed and may surface new failures, so it's a task of its own under TASK-149, not done here.
**Undoing it:** drop the scenario and the hold; `zloop.sh` back to the one-run binaries alone.

## 54 stale In Progress tasks triaged: 46 closed, 7 back to To Do (2026-11-02)

Decided with the user: criteria are ours, so a task whose work landed, or whose rest isn't worth doing as written, is closed, and its final summary says what was decided not to do. Unmet criteria stay unchecked. Back to To Do, as real work left that nobody is on: TASK-42.11, 79.82, 91.15, 111.1 (the pgbench decay over five runs, the one task carrying it), 149.2, 149.14, 150.6 (now depending on TASK-153.9); TASK-60 too, waiting on 60.1. Left alone: TASK-158, uncommitted work in worktree t3-9b5982f8. Also closed: TASK-153.6, done by fafefbac7 and 106b6128d, and TASK-153.10, 112.5 and 109.1, about evacuation, which fafefbac7 removed.
**Instead:** keep each open until its criteria are met as written.
**Undoing it:** `backlog task edit <id> -s "To Do"`; each task's notes and summary say what was left.

## Unmerged branches became `archive/` tags, not deleted outright (2026-11-02)

No long-lived branches now (AGENTS.md). Branches already in main went. Unmerged ones, untouched for two days and checked out nowhere, became tags `archive/<branch>`: 50 local, 5 on GitHub, where only main is left. Kept as they were: branches checked out in a worktree or changed in the last two days, as other sessions may be on them.
**Instead:** delete them outright, losing their commits once the reflog expires.
**Undoing it:** `git branch <name> archive/<name>`, then delete the tag.

## Truncate on a full disk: one path zeroes the tail (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Decided: every non-DAX growth takes one path that zeroes the tail, with one owner of file size (TASK-153.4). Done (2026-11-03): a write raises the size through `txn_grow_size`, and a clone and an allocation zero first. Each writes the block only if it isn't zeros already, so a writeback's growth takes no block it didn't reserve. The cut still zeroes eagerly when it has room, so that what a mapping of the last page shows past the end is zeros; with no room, it no longer matters to the data.
**Instead:** a mid-block shrink could return `NoSpace`, partly reversing TASK-60.
**Undoing it:** the single path stays; only the shrink's error changes.

## Corrupted file data: the host's checksums catch it (TASK-153.11)

A byte damaged in storage, by a bad disk or a bad upload, must make a read fail rather than return the wrong byte. Decided: rely on the host's CRC32C in `extent_frame`, confirm every host read path checks it, and close gaps there. The guest doesn't check.
- Fxfs's own data checksums are read only at journal replay, as in Fuchsia's code, and DAX stores skip Fxfs's write path so have none.
- Pages already in the DAX window are read straight from memory, so nothing can check them there.
- Still uncovered: data on a host's disk before it's sealed, and pages already in memory.

**Instead:** the guest checks too, costing reads on the fast path.
**Undoing it:** adding guest checks later doesn't conflict with anything this does.

## A framed file's raw parts are read whole and checked, as compressed ones are (TASK-153.11)

A part zstd doesn't shrink is stored raw, and reads took its bytes in place, never checking its CRC: a flipped byte in an incompressible extent, local or a fork's base's, read back wrong, and cleaning and seals copied it under a CRC of its own. Decided: a raw part is read whole, checked, and kept with the parts unpacked lately, in the buffer it was read into. Measured (pagein-bench, M-series, 256 MiB, 3 pairs of runs): a raw part's page-in, which reads it whole anyway, +10-30 µs of CPU on about 185 µs; the CRC is 16 µs a MiB. A cleaning pass of 4 KiB raw extents (store-bench `cleaning`, in memory) 15 ms → 23-37 ms for 23 MiB, as each 4 KiB read reads its 1 MiB part. The uploader also checks every file it puts as it is (`read_object`): about 0.5 ms for a 32 MiB segment.
**Instead:** a CRC per 4 KiB block in the table, a format change, so that small reads of raw parts read only their block.
**Undoing it:** `Source::locate` back to `Part::Data` for raw parts of framed files.

## A fork of an extent mapped writably holds it as of the last commit (TASK-153.3)

Today such a fork gets `EBUSY` (TASK-150.7). Decided: the fork holds the extent as of the last commit, as the spec says, so the host half of TASK-153.3 goes ahead. This reaches only a VM process's own raw FORK: production forks use `fork_quiescent`.
**Instead:** the fork also holds the mapped, uncommitted bytes.
**Undoing it:** the fork path in TASK-153.3 copies the mapped bytes as well.

## A device write to an extent not known to be open is taken, not refused (TASK-153.3)

The task's direction had such a write retry rather than reopen its extent. Decided: the store takes it, and nothing records the extent open, ever again: the open extents are those minted since the last seal (`OpenExtents`). Plain writers (multi_write, the journal, layer files) have no retry, where `SealedExtent` is EIO, and DAX's is dax.c's `gate != seals` already.
**Instead:** refuse it in the guest's device, and give every writer a retry.
**Undoing it:** a check in `write_with_opts`, and retries at each writer: `dead-ends/refusing-writes-to-extents-not-known-open.md` has why not.

## Shared blocks of an open extent never go through its region (TASK-153.3)

A clone's blocks in an open extent read `Shared`, and get a range of their own, read-only, never a slice of the extent's region: so a region of an open extent is never mapped read-only. Costs a MAP of their own for reflinked small files' blocks until their extent seals.
**Instead:** keep the checks in `region_open`, `map_to` and `new_slice` that let a reader and a writer share a region (TASK-99, 140, 44.21, 150.11).
**Undoing it:** `fxfs_dax_new_slice`'s one line, and those checks back.

## NoOpenUnderLock.cfg asks what it can now (TASK-153.3)

Its rule, region_open under the region's lock, went with the look it guarded: a reader and a writer don't share a region. The file is back, taking away the rules that keep them apart, and `MapUnderAnOpen.cfg` asks for the race itself, a reader's MAP in flight as a writer opens the region.
**Instead:** put the region's lock back in `region_open`, dead, for the rule to mean what it did.
**Undoing it:** the lock back in dax.c and the rule in the spec.

## 106b6128d's lag under a PTE is fixed on refactor/open-once (TASK-153.3)

A write through a region left a fault's PTE of the page it lags, read stale: found here, verifying the merge, so fixed here, its own commit, with a mutant.
**Instead:** a task of its own on main.
**Undoing it:** revert 157417ad5.
