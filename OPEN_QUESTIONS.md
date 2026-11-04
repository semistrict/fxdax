# Open questions

## The nightly needs GCP credentials in GitHub (2026-11-02)

`.github/workflows/nightly.yml` never ran: its YAML didn't parse (fixed in a1e079306). Now it parses, it needs the repository's `GCP_WORKLOAD_IDENTITY_PROVIDER` and `GCP_SERVICE_ACCOUNT` secrets and its `FXDAX_GCE_PROJECT` and `FXDAX_NIGHTLY_BUCKET` variables, none of which is set. Granting GitHub access to the GCP project is yours.
**Recommended:** a workload identity pool for `semistrict/fxdax` and a service account allowed to make and delete GCE instances and write the nightly bucket.
**Until then:** each nightly fails at its auth step; TASK-69's runs stay manual.

# Decisions

## The host's copy-on-write DAX protocol once writable DAX is deleted (TASK-164.7, 2026-11-04)

Writable DAX is deleted from the host (vendor/firecracker's extent device, crates/fxdax, physmem, extent_store). Decided, where the brief left it open:
- **Daemon wire `WINDOW`:** no flag. `args[1]` is the private pages a window holds; a writer's window must copy on write (`args[1]` ≥ 1), a follower's must not (`args[1]` 0), `EINVAL` otherwise; the codec refuses any flag, and more private pages than the window has. The client's `create_dax_window_in_host_memory` takes `private_max` (0 for a follower); `create_copying_dax_window_in_host_memory` is gone. In-process, `ExtentStore::create_dax_window` (refusing every store) stays beside `create_copying_dax_window`, for followers and tests.
- **Retired numbers, not reused:** device feature bits 1, 9, 10, `MAP` flag bit 0, request 15 (`T_SEALED`); daemon requests 29 (`SEALED`), 107 (`SEAL`), 102 (`STAGE_PROTECT`), `MAP` flag bit 0, `MAPPING` flag bit 0, `WINDOW` flag bit 0. The config drops `seal_asked`: `private_max` follows `committed`. `STAGE_MAP`'s `args[3]` (its protection) is unused, ignored as words a kind doesn't use are. `STAGE_COUNTS`/`ACK` say four counts: write-protects went with seals.
- **`Protection` is gone from `SecondStage::map`:** every map is read-only; only `make_private` makes a page writable.
- **Stats and the control API:** `commit-write-ns`, `commit-written-bytes`, `commit-seal-*`, `stage-write-protects`, `stage-maps-open`, `open-alloc-*` are gone; `GetStatsResponse` reserves fields 16, 17, 19. scripts/bench-report.py reads `commit-seal-ns` unconditionally and needs the same change.
- **`FXDAX_CHECK_SEALED_READS` is gone:** it compared reads with what commits wrote of open extents' memory, which no longer exists.
**Undoing it:** none of this has a flag; restoring writable DAX is reverting the change.

## Copy-on-write DAX's private pages are the VM process's memory, laid out as the window is (TASK-164.1, 2026-11-04)

Each private page was a page of the tenant's arena, mapped in the VM process on its own: a mapping each, as the arena's frames aren't laid out by window offset, so the second stage's run budget (about 30k) trimmed and reinstalled them by the thousand; r544 held 110,857 and its fault thread trimmed instead of lifting a vCPU's restriction (`KVM_RUN` kept failing with `EFAULT`, exit 159). Decided: a window that copies on write has the VM process's private memory (`dax::uffd::PrivateMemory`), a memfd as large as the window, mapped shared at the window's offsets wherever nothing is installed, only private pages filled (`UFFDIO_COPY` through a second view, holes elsewhere so faults still go to the stage). `STAGE_PRIVATE` (108) has the VM process copy the source page (pread of the arena it holds) or zeros in; an unmap or map over it punches the hole. Adjacent private pages and the memory around them are one mapping; trims never touch them. The daemon keeps only their offsets and count; `private_max` is still pinned against the tenant's arena as the window is made.
**Cost:** the private pages' memory is the VM process's (its memcg), no longer in the arena's bytes: a tenant's limit no longer bounds it, `private_max` does.
**Instead:** (1) private frames allocated next to their window neighbours' (`alloc_after`): best effort, and scattered pages (pgbench's) still cost a mapping each; (2) a per-window physmem arena laid out by window offset: chunk-granular accounting charges 2 MiB per scattered page; (3) charging the memfd against the tenant's cap (physmem `charge`): possible follow-up.
**Undoing it:** `Window::copy` back to `physmem::alloc` + `map_run`, and `leave_out` to anonymous memory.

## The control socket is owner-only (TASK-159, 2026-11-04)

The control socket (`FXDAX_EXTENTD_GRPC`) followed the umask, world-connectable under the jailer's umask 000, and grants bound their tenants' sockets then chmodded them, a window at the umask's mode. Decided: the control socket is 0600, the daemon's user's, whatever the umask; a tenant's socket is 0660 (its jail's uid and gid) from the moment it appears at its path. Both go through `extentd::bind_given`, which binds in a private 0700 directory, sets the owner and mode there, and hard-links the socket into place (`EEXIST` if anything is there). No code or doc gave a group the control socket; every caller found runs as the daemon's user or root.
**Instead:** a configured group (a gid, mode 0660) for a VM manager of another user.
**Undoing it:** the mode in `fxdax-extentd.rs`'s call of `bind_owner_only`.

## DAX writes Fxfs's own blocks in place only if their record is raw (TASK-160.22, 2026-11-04)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); no window writes in place, `txn_map_write` and `window_access` are gone, and `in_place` serves `overwrite` alone.
A window wrote in place any blocks of an open extent the file alone held, whatever their record said. Blocks under a checksummed record (a growth's zeroed tail, ordinary writes) then held bytes their checksums didn't, which journal replay checks; blocks under an allocation's record read as zeros to Fxfs whatever the window stored. Decided: one rule, `in_place`, used by `extents_in`, `txn_map_write` and `overwrite`: in place only if the record is raw and nothing shares the blocks. Such blocks in an open extent are `ExtentAccess::Shared` to the window (read-only, the extent open), and a window's first write moves them once.
**Instead:** write in place and drop the record's checksums or allocation bitmap as the window writes: a transaction per first store.
**Undoing it:** `window_access`'s `InPlace::Recorded` arm back to `Open`.

## A layer's records cross one trust boundary, checked as they're read (TASK-153.12, 2026-11-03)

Layer records were trusted as read, then checked piecemeal where used. Decided: each item a layer file holds is checked as `take_item` reads it (`LayerKey::is_valid_item`), with the predicate the journal's mutations pass at replay (`ObjectValue::is_valid_for`, `ExtentValue::fits`; `AllocatorValue::is_valid_for` for the allocator's), and each key against the one before as an iterator advances (`follows`: after it, overlapping none). An item no layer may hold fails the read, a seek to it, fsck (`MalformedLayerFile`, also for keys out of order or overlapping, which fsck named on its own before) and the compaction meeting it. Cost, `layer_read_cost` on one GCE n2-standard-4 per comparison, three hosts agreeing to 1%: a scan 144.9 ns an item before, 157 with the item check (+8%), 165 with the order check too (+14%); seeks unchanged (1,050 to 1,200 ns either way). On an M-series Mac, under load, about +10%. Landed for both: without it, a record with checksums over its key, or keys overlapping, read back differently in 10 of 16 cases of `records_no_layer_may_hold_fail_what_reads_them`, with nothing saying so; the cost is on scans (compactions, fsck, a file's extents in turn), not on the seek every read starts with. Where the 12 ns of the item check go is unexplained: neither its two divisions (taken out, no change) nor, measured on arm64 only, the check alone (3 to 5 ns) account for it; moving the error paths out of line (`#[cold]`) wasn't measured, as gcloud's credentials expired. A seek still trusts order, as a binary search must: a key out of order may be missed by a point lookup, and only a scan finds it.
**Instead:** (1) keep checking where records are used: piecemeal, and wrong data where no use checks; (2) drop the order check, saving 6%: merges then trust order unchecked; (3) a check function each tree gives its layers, as `merge_fn`: threads one through every layer's opening, where a default method on `LayerKey`, downcasting to the store's value type (`dyn Any`, resolved at compile time), needs nothing threaded.
**Undoing it:** take the checks out of `take_item` and `take_next`; the predicates stay the journal's.

## Corruption is EUCLEAN and EBADMSG, not EIO; one errno table for both VFSs (TASK-160.21, 160.12, 2026-11-03)

Fxfs raises `Inconsistent` and `IntegrityError` on purpose now (TASK-153.11, 153.13), and both VFSs said EIO for them. Decided, in the one table both now call (`fxfs_posix::errno`): `Inconsistent` is `EUCLEAN` and `IntegrityError` `EBADMSG`, ext4's and XFS's `EFSCORRUPTED` and `EFSBADCRC`, logged. No caller depends on EIO for them: fs/fxfs's C compares Fxfs's errnos only with ENOENT, ENOSPC, ENODATA, EXDEV, EBUSY, ESTALE and the like, and no guest or host test expects EIO of a corrupt image. The rest of the table, where the two had drifted or said EIO: `AlreadyBound` EBUSY, `OutOfRange` and `InvalidVersion` EINVAL (ext4's mount of features it lacks), `NoKey` ENOKEY and `InconsistentEncryptionPolicy` EXDEV (fscrypt's). An error is classified by its outermost Errno, else its outermost FxfsError, context first; Fxfs's `matches` (root cause) agrees but for an FxfsError added as context. What only looked like corruption no longer says it: a manifest that doesn't parse or names no base is `InvalidArgs` (EINVAL; the guest's garbage-manifest check said EIO, and says EINVAL now), changes after a failed batch of unlinks fail with EIO, and a DAX write giving up on seals gives the seal's error (EIO). On the host, the uploader's failures keep `extent_remote::Error` (`Failure::Remote`) and map in one place (`Failure::errno`): `Displaced` is EROFS, as `Fenced` is, where it was EIO; a branch of a newer format (`Format`, TASK-141.2) ENOTSUP, until the host is upgraded; object storage's `Lost` and `Corrupt` stay EIO, doc-4's table having no other status for a store failing to read.
**Instead:** EIO for all corruption, as before (callers can't tell a bad disk from bad metadata); `EFSCORRUPTED` on the device's wire too (doc-4 would need a status).
**Undoing it:** the two rows in `crates/fxfs-posix/src/errno.rs` (`of_fxfs`) and their test rows.

## A DAX write(2)'s move commits after its bytes, as XFS's copy on write does (TASK-153.5, 2026-11-03)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); `fxfs_dax_begin_write` and `DaxWrite` are gone, and copy-on-write writeback commits after its bytes through `fxfs_write_many`.
A write(2) over the file's data moved its blocks and committed before its bytes were copied, so a write that failed after the move (a MAP failure, a seal, a killed writer, a copy cut short) left the new blocks' zeros where the data was (TASK-79.106, 119.13, 150.8, `map_failed_writing`). Decided: the move stays an uncommitted Fxfs transaction (`fxfs_dax_begin_write`, `DaxWrite`) from the iomap to `iomap_end`, where it commits out of the gate once the bytes are in, or is dropped, leaving the file as it was. The write holds its file's semaphore all along, and waits for faults past their iomaps first. Faults and zeroing still commit first. pgbench-dax measured equal: 826.3 against main's 834.3 tps over 25 runs a side on one host, the rounds alternating around 1 (docs/measurements/pgbench-dax-commit-after-bytes-2026-11-03.md). Landed for what it removes from write(2): bytes in hand before a move (-ENOTBLK), the -ESTALE retry after one, a second move after a seal, the second unmap of TASK-153.3; and a failed write can no longer zero data, which guest/sim's `failures` now checks.
**Instead:** keep committing before the bytes, with the retries that mend a failure after: the same tps, and more ways to lose data.
**Undoing it:** revert the TASK-153.5 commits (dax.c, fxfs.h, fxfs-linux, fxfs-posix's `dax_begin_write`, the sim model); the guest checks `write_killed` and `map_many_failed` go back to expecting the old behaviour.

## What a store may be asked to do is the store's, fixed as it opens (TASK-160.10, 2026-11-03)

Two holes were real: ResizeStore grew a point (the daemon opened it without its remote, which alone says it is one), and a failed listing of a store's public markers opened a public base private and writable. Decided: `ExtentStore::access()` (`StoreAccess`: writer, follower or point, and public) is fixed once the store is open and attached, and its methods check it: what changes what it holds (write, free, commit, map writably, clean, take over) needs a writer's store that isn't public (`EROFS` for a follower or point, `EPERM` for a public base); a resize needs a writer's, public or not, as no fork shares a capacity, but a point's forks take its; forking, making public and attaching a base, which change no extent, need any but a follower's, so a point may still be made public or have a base attached. A public base now refuses writes at the store too, not only a VM's open of it. Resizing a closed store attaches its remote, as every other daemon open does, so it takes the branch's lease (`ESTALE` if a later placement holds it). EROFS maps to FAILED_PRECONDITION. A marker listing that fails fails the open with `EIO`, logged. The default tenant is a real `Tenant`, nameless and unregistered for limits, with a policy of its own (roots, names branches, follows public bases).
**Instead:** keep each caller's checks and fix the two holes alone; or record point-ness locally, which would be a second copy of what the bucket's record says.
**Undoing it:** `StoreAccess::may_write` lets public bases through; `stores::resize` back to `open_without_profile`.

## CI runs scripts/test.sh, a part a job; open bugs' failures are listed, not failed on (TASK-160.27, .28, 2026-11-03)

CI ran `cargo nextest --workspace` alone. Decided: CI runs `scripts/test.sh`, which now has parts (`FXDAX_TEST_PARTS`: clippy, workspace, shuttle, scripts, and guest on Linux), as four jobs at once: the workspace, the Shuttle suites, the guest tests' own (as root, as on GCE: some mount loop devices), and the rest. The guest part runs guest/tests' own tests, both feature sets (vm.rs, script_vm.rs ran nowhere); scripts runs the mutant runners' `--check`. Every hand mutant's patch applies with no fuzz now, or fails `--check` (7 needed fuzz, one was malformed: refreshed). Open bugs' failures go in a file next to what meets them, `guest/sim/known-failures` (a regex of zloop's failure line) and `crates/fxfs-posix/known-panics` (a panic's place, which `corrupt_tests` counts apart); both are read on every run, CI's too, not only by the hunt, so that one bug fails nothing until its fix takes its line out. A nightly `hunt.yml`, CPU only: zloop over 20,000 new seeds (4 shards), the model and corruption tests at 50x (4 shards, seeds from the run's number); each night some 6 runner-hours, free on a public repo.
**Instead:** one known-failures file for both (their formats differ); known signatures only in the hunt (CI's 3,000 seeds then fail as often as an open bug shows: TASK-150.21's WARN, about one seed in 6,000, until patch 0012 fixed it); a hunt on GCE (needs the nightly's credentials, unset).
**Undoing it:** CI back to its `cargo nextest` step; drop `hunt.yml`; empty the two files. guest/sim's lists none now.

## A branch record's flags stay on the wire; it is decoded once into a kind (TASK-160.14, 2026-11-03)

A record's six flags (origin, forked, deleted, point, forking, claimed) were read as flags in four modules. Decided: they stay the bytes in the bucket, unchanged (FORMAT 1), and are decoded once into `record::Record { format, owner, kind, deleted }`, with `Kind` one of Root, Own, Fork, Point, Forking, Claim. Any other combination is refused as corrupt (52 of the 64). What a record allows is a query on it (`writable`, `is_child_of`, `may_drop`, `branch::lost_is_ok`), and every create or update of a record or a lease goes through one compare-and-swap loop, `branch::transition`. `branch::ForkedAt` is gone: `delete_fork` takes the `Kind` its child's record is or would be, and fxdax's claim holds that kind once its fork's segment goes. The suspected bug (a direct fork and a fork of a point at one segment, given one child ID, taking each other's records) was real until TASK-153.9's owners; a test now shows both that it no longer happens and that it does with `records_owned` off.
**Instead:** put the enum itself on the wire (serde's tag), so that no illegal record can be written at all; but decoding already refuses them, and the flags are what doc-7 describes.
**Undoing it:** fold `record.rs` back into `layout.rs` with public flags; nothing on the wire changes either way.

## The daemon asks other guests to seal when a writable MAP is refused (TASK-153.15, 2026-11-03)

**Superseded (2026-11-04):** writable DAX is deleted (doc-13, TASK-164); with no open pages there is nothing to seal for, and `F_SEAL`, `seal_asked`, `SEALED`, the daemon's `SEAL` and `extentd/seals.rs` are gone.
Only a guest can seal its open extents, whose pages count against a budget its daemon's VMs share. One idle VM's open pages could take the budget, and another VM got `ENOMEM` from writes its own seals couldn't make room for. Decided: when a writable `MAP` is refused, the daemon asks the guests holding the most open pages to seal. It asks those of its tenant first, enough to cover the `MAP`. It waits for their answers, holding no thread, 10 s at most, then tries once more. This uses a device notice (doc-4's `F_SEAL`, `seal_asked`, `SEALED`), shaped like the resize notice. The guest seals with an ordered commit. Asks are made only under pressure, not on a timer for idle VMs: the most-held ranking already picks the idle VM, and a timer would seal VMs nobody needs room from. A guest that has been asked never waits for another guest's seal, which rules out a cycle of waits. A guest that doesn't answer within 10 s isn't waited for again until it does.
**Instead:** (1) a per-tenant share of open pages: a VM still can't go past its share while others sit idle, and the shares need sizing. (2) Evict open pages to the store and fault them back: the daemon would have to take writable pages away from a running guest. (3) Also ask idle VMs on a timer.
**Undoing it:** in `Connection::map_writably`, stop calling `seals::made_room`; take `F_SEAL` out of the device's features. The guest then never sees an ask.

## The extent store's audit runs in every test build that links it (TASK-160.16, 2026-11-03)

`State::audit` recounts the ~15 byte counters from everything the branch holds each time its state's lock is let go, in builds with the `audit` feature. Decided: `shuttle` implies it, and `extent_store` turns it on for its own tests by a dev-dependency on itself. Cargo unifies features in one build, so a workspace test run (`cargo nextest run --workspace`, `scripts/test.sh`, GCE) audits the stores of fxdax's and the other crates' tests too: more checking, at the cost of a walk of every extent per critical section. Only branches holding at most 4,096 extents are audited (`AUDITED_EXTENTS`). Unbounded, fxdax's `counting_followers_walks_no_extents`, which writes a 32,768-extent base one extent at a time, ran past an hour on GCE. Release builds and the daemon never audit.
**Instead:** `cfg(debug_assertions)`, which also audits debug daemons and guest tests, whose stores are large; or an opt-in each test calls, which misses the ones that don't.
**Undoing it:** drop the self dev-dependency (the Shuttle and lib tests still audit), or `audit = []` and the `cfg`s.

## The three footer replays stay three for now (TASK-160.15, 2026-11-03)

TASK-160.15's bug is fixed at the source: footers free only extents a commit held (`layout::held_frees`, which delta replay uses too), so local rebuild spends what the store spends. Merging the replays into `extent_frame::segment::Replay` is left for later. They differ on purpose: remote recovery spends only the frees of extents it held before the segment, takes the largest `next_sequence`, and has its own continuity rule and layers. Each difference needs its own decision, and TASK-153.7/.8/.9 changed the footers of both crates just now (Footer.forks of Child).
**Instead:** merge them now, choosing one rule for each of those differences, across a format change in flight.
**Undoing it:** nothing to undo; the merge is TASK-160.15's AC #2, still open.

## The journal's end is in the root, set by each sync's barrier (TASK-153.13)

A flip mid-journal read as its end, losing synced transactions unsaid. Decided: the root records how far the journal was written when a barrier set it (`Root::journal_end`), and a replay ending before there is refused as corrupt; past it, a torn tail is still a torn tail. A sync's barrier sets it: its journal write's `WRITE_COMMIT` carries the root (`WRITE_ROOT`, new on the wire), or its flush becomes a commit of the root. Cost: no request more; the root's ~50 bytes in the sync's request and the host's commit record; barriers setting roots take turns (they did at the device already). A synced write whose data the journal's checksums find damaged now refuses the mount, where it rolled the file back.
**Instead:** a durable marker only on `sync`/`syncfs` (fsyncs unprotected), or an extra `COMMIT` per fsync (a round trip each), or no marker, leaving it to the host's CRCs, which bugs in ordering get past.
**Undoing it:** drop the replay check; the root's field and `WRITE_ROOT` can stay unused.

## A transaction that fails to apply still stops the journal (TASK-153.14)

A live transaction whose apply fails (a corrupt image's allocator, or a bug) stops the journal: writes fail, reads go on, a remount replays without it. Decided to keep that: it is ext4's errors=remount-ro, and the failure means memory contradicts itself, which writing on would spread. Making apply unable to fail needs a dry run of every mutation against state the transaction's own earlier mutations change, or undo for each kind: a second apply to keep in step, to go on writing on metadata known to be wrong.
**Instead:** check-then-apply under the writer's lock, failing only the transaction.
**Undoing it:** nothing to undo; that is new work.

## A branch's owner is who made it, not a tenant (TASK-153.9, 2026-11-03)

Every branch record now names an owner: a random number chosen by whoever made the branch, unique to that making, and the same in every copy of the record whoever writes it. A fork's host picks its child's owner when it claims the child, and the fork's footer carries it. Two forks naming one child no longer both win, and a fork's cleanup deletes only its own child (TASK-79.82, AC1). Decided: the owner says who made the branch, not which tenant it belongs to. Tenants stay authorized per host by their directory and the VM manager's `CreateStore` (TASK-154).
**Instead:** put the tenant's name in the record, and let a tenant's `OPEN_FORK_FROM` take a point it owns by naming it, which would spare the VM manager a `CreateStore` before a restore on another host. But nothing makes a tenant name mean the same on every host, and it would bring back the tenant-named-branch check that TASK-150.4 removed.
**Undoing it:** a `tenant` field next to `owner`, set by `CreateStore` and passed down from parent to fork, plus a check in `extentd/server.rs`'s `open`.

## A segment's framing ends itself (TASK-153.8, 2026-11-03)

Five bugs came from a framing being ended by whatever came next: a commit's flush, a flush with nothing to commit, `close_now`, closing the branch, or a fork (TASK-41.1, 79.69, 79.99, 92.16, 150.2). Decided: every framing now runs on its own thread, makes its file durable, and finishes itself under the store's state lock. Everything else only waits for it: a commit's flush waits for the segment that commit closed, the uploader for segments whose commits are durable, `close_now` for its own, and forks and closing the branch for all of them. A side effect: the raw file of a segment a commit closed now goes at the first flush or close after both its framing and its commits are durable, not with that commit's own flush.
**Instead:** keep handing each framing to whatever comes after it, as before.
**Undoing it:** revert a38829973.

## Writers declare what's in flight in their keys (TASK-153.7, 2026-11-03)

The collector decides whether an unreferenced object may still be needed from its key. An extent's key names the writer that uploaded it and the segment that will name it. Decided: keep that key as the writer's declaration, and make it exact. Keys always name their segment now (old keys without one are no longer read), and one function, `Pass::may_land`, decides for segments and extents alike.
**Instead:** a separate declaration object or lease field. It would have to land before the objects it covers and be read after the listing, it needs garbage-collecting itself, and it says nothing the key doesn't (`dead-ends/collector-in-flight-declarations.md`).
**Undoing it:** nothing to undo; a declaration object would be added alongside.

## Bucket branches say their format (TASK-141.2, 2026-11-03)

Every branch record now starts with `layout::FORMAT` (1). A reader refuses a newer format with `Error::Format`, naming the branch and the format, before reading or writing anything else of it, and the collector's pass stops without deleting. Framed objects already end with their frame's format (`fxdaxob1`); a newer one is now reported by name instead of as corrupt. The old-lease decoding from before fencing tokens is gone (no migration, AGENTS.md). Store files went to `fxdax extent store 5`, since footers' forks changed.
**Instead:** a format field in every object, footers and checkpoints included. Every reader reaches a branch's objects through its record, so the record is where the check goes.
**Undoing it:** drop `Record::format` and `Record::decode`'s check.

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

## TASK-150.21 has no guest check: its steps need a staging extent the host doesn't hold (2026-11-03)

guest/sim's `pmd_entry_invalidated_mapped` reaches fs/dax.c:416's WARN by failing the MAP of the staging extent's region after a seal, so that a fault's move into it, a seal shutting the gate, leaves the page a hole. A guest has no fault injection for that MAP, and the only other way, a seal between the restage and its MAP, is a race. Decided: no guest check; the sim's scenario and mutant carry it, and the DAX guest suites run patch 0012's path on a real kernel at every write that says its pages are new.
**Instead:** a module parameter failing the restage's MAP (`fxfs.dax_fail_restage`), and a guest check of the same steps.
**Undoing it:** add the parameter in `fxfs_dax_restage_work` and port the scenario to test_fxfs_guest_checks.rs.

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
