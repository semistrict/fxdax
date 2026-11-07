---
id: TASK-149.36
title: >-
  Metadata in the model test: mode, owner, group, times, special files,
  fallocate, names, readdir, xattr limits, statfs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 05:20'
updated_date: '2026-11-07 08:28'
labels:
  - testing
  - fxfs
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 659000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-222's FUSE mount found fxfs-posix's setattr zeroing a node's mode when one of mode/owner/group alone changed on a node without POSIX attributes (a new volume's root): a bug in basic POSIX metadata no randomized test could find, because the fxfs-posix model test (model_tests.rs) only checks size, links, xattrs and bytes. Its gaps, surveyed 2026-11-07: mode/uid/gid never changed or checked; atime/mtime/ctime never checked (setattr only truncates, plus a forced mtime in forget); every file made S_IFREG, no FIFO, socket or device, rdev unchecked; fallocate (Fxfs::allocate) never called; names are single letters: no 255-byte names, multibyte UTF-8, casefolding directories (which Fxfs has), or invalid names; readdir only read whole from Index(0), never resumed with After(name), with names removed between; xattr values at most 8 KiB and 3 names: no empty values, none near 64 KiB, no many; the root directory, the one node Fxfs makes without POSIX attributes, seldom an operation's target; statfs never held to the model; snapshots and write_dirty never in random sequences. The model must hold each node's attributes as a VFS on Linux would see them, starting from the defaults Fxfs reports for a node without POSIX attributes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sequences change random subsets of mode, uid, gid, atime and mtime on any node, the root and directories included, and every check holds each node's mode, uid, gid and rdev to the model, across remounts and power cuts
- [x] #2 Times are checked as Linux keeps them where Fxfs promises it: a set time reads back; writes, truncations and changes of a directory's names move mtime and ctime no earlier than before
- [x] #3 Sequences make FIFOs, sockets and character and block devices with rdev, which read back as made, and fallocate files, the model holding the size and zeroed bytes
- [x] #4 Names include 255-byte and multibyte UTF-8 names, some directories casefold, and readdir is also read resumed after a name, with names removed and added between its reads, each name listed once
- [x] #5 Extended attributes include empty values, values near the largest Fxfs takes, and more names
- [x] #6 statfs is held to what the model's files and xattrs must at least take
- [x] #7 Each new kind of operation is shown to find a bug: a mutant of the code it reaches, red, recorded in the notes; the fxfs-posix suite passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Model every node's attributes (Node: mode, uid, gid, rdev, times as Time::Is/Since, xattrs) from the root's defaults; check them on every lookup, readdir and file check, and st_blocks exactly.
2. New ops: Make (FIFO, socket, char/block devices, files and dirs with owners and permissions), SetAttr (random subsets of mode/uid/gid/atime/mtime/ctime on any node, the root first), Allocate (fallocate; Data.allocated, checked against Fxfs's overwrite extents and what a DAX window maps), ReadSome/ReadOn (readdir resumed after a name, Cursor), Casefold, BadName (non-UTF-8 names refused), NodeXattr (set/create/replace/remove on any node), Statfs.
3. Names in two alphabets (SPELLINGS): letters, other cases, ß/SS, an ignorable character, 255-byte ASCII and multibyte; casefolding directories fold them (FOLDED, Model::spelled).
4. xattr lengths: empty, around the inline limit, kilobytes, around and past MAX_XATTR_VALUE_SIZE; 255-byte names.
5. statfs held to the model's blocks at least (distinct page Arcs, xattrs outside records) and its objects.
6. Keep cuts, remounts, tiny devices and failing devices working: had_it_worked, stopped_short and cut candidates relax times and adopt what's allocated.
7. Fix what it finds, per AGENTS.md; mutants under crates/fxfs-posix/mutants; hunts with FXDAX_MODEL_CASES.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
What the model test checks now (crates/fxfs-posix/src/model_tests.rs, module docs): every node's mode, owner, group, rdev, times and xattrs (Node), the root's from Fxfs's defaults, at every lookup, readdir and file check, and st_blocks exactly (written and allocated pages, xattrs kept outside their records); new ops Make, SetAttr, Allocate, ReadSome/ReadOn, Casefold, BadName, NodeXattr, Statfs; names in two alphabets (SPELLINGS: letters, A, B+ZWNJ, Straße/STRASSE, 255-byte ASCII, 255-byte multibyte), casefolding directories folding them (FOLDED via fxfs_unicode::casefold_cmp); xattr values empty, 254..258, KiB, 63984..64001, a 255-byte name; statfs at least the distinct page blocks and outside xattr blocks, and the objects; what each file has allocated (Fxfs's overwrite extents) and what a DAX window maps (dax_extents == pages written).

Times, decided after reading Platform::KEEPS_DIRECTORY_TIMES and the host (crates/fxdax/src/filesystem.rs sets data times with setattr after writes): the model runs the host's platform (false), so Fxfs times directories' names, links, renames and ctimes itself: those are held Since(the clock before the op), and a set time Is() exactly; a file's data times (writes, truncates, clones, punches, allocations) and queued unlinks' are the VFS's: no earlier than before (Time::no_earlier). Set times are kept in the past so that "no earlier" holds. The guest's platform (true) is left to TASK-149.40.

Bugs found, each fixed in a commit of its own:

1. readdir listed FIFOs, sockets and devices as regular files (56fdf6b12, on the refactor 288541fd2). Found: the first run, [Make FIFO] on any seed. Blind spot: every node the model made was S_IFREG, and readdir's types were compared with the entry kinds the model invented, never with what attr says; the host's FUSE entries() even collapsed every other type to File, and its test fuse::directories asserted DT_REG for a FIFO. Class test: the model's readdirs (holds_names, read_on) compare each listed type with the node's type; red with the bug back (mutant readdir-lists-special-files-as-files: every_cut_of_a_short_sequence and tests::readdir_lists_special_files_as_what_they_are). Look around: the host's entries() mapped specials to File (fixed, fuse test corrected to DT_FIFO); the guest passes the mode to fs_umode_to_dtype (fine once the mode is right); link and exchange take descriptors only to name entries (fine). Structure: two readers of a node's type (attr_of, readdir via the descriptor) became one, node_type, refactored first (288541fd2). Architectural: Fxfs's child records say only file/dir/symlink, so readdir now looks up each file's record; entries saying the type (a new ObjectDescriptor, a format change) would spare it: recorded in OPEN_QUESTIONS.md for the user.

2. fallocate of part of a block, then any growth of the file, failed EIO "no extent" (9796f1ad7). Found: the first hunt, [Create, Allocate {0, 1}] then a cut; reproduced without cuts (allocate 0..1, truncate up). Blind spot: no randomized test ever called Fxfs::allocate; upstream's tail zeroing reads the block straight from its extent ("not strictly incorrect to change uninitialized data"), which a block device allows and fxdax's write-once extents (doc-4) don't: an allocated block nothing wrote isn't on the device. Class test: Allocate in random and short sequences, with every growth path after it; red with the bug back (mutant the-tail-is-read-from-its-extent: a_file_allocated_part_of_a_block_grows, tail::tests::test_a_file_allocated_part_of_a_block_grows). Look around: readers of a file's blocks that bypass the read path's unwritten rule: align_buffer reads through read_aligned (fine); journal and super-block readers read no file data; DAX extents_in handed the window blocks an allocation left unwritten: bug 3. Structure: the tail now reads through the one read path (StoreObjectHandle::read_aligned_locked, what read_aligned_unchecked delegates to), and DataObjectHandle::read_and_decrypt, its last use gone, went: a file's blocks are read in one place.

3. A DAX window would map blocks an allocation left unwritten (c90116747). Found looking around bug 2. Blind spot: dax_extents were never compared with the data. Class test: holds_extents checks after every data op and at check_all that the pages a window maps are exactly the pages written; red with the bug back (mutant dax-maps-blocks-never-written: a_file_allocated_part_of_a_block_grows, dax::tests::test_extents_in_leaves_out_what_an_allocation_left_unwritten). Guest DAX of fallocated files isn't run on GCE here.

Not bugs, as Fxfs means them (the model learned them): an xattr kept outside its record stays there however short (upstream's set_extended_attribute_impl), so st_blocks counts its block; writes, clones and punches end a page's overwrite (allocated) mode, as fxdax writes everything anew; a refused allocation may leave the file grown to the range's end (Fxfs commits the size first; decision in OPEN_QUESTIONS.md); an xattr replace of a missing name on a full device says ENOSPC before ENODATA (the transaction is reserved first), changing nothing either way: the model takes either. Oracle bugs fixed on the way: a partial fill of zeros told from holes by whether a block holds the page.

Mutants (crates/fxfs-posix/mutants, mutants.sh -c 96, all red; the first test to fail): setattr-zeroes-what-it-isnt-given (the_roots_attributes_change_as_any_nodes), setattr-loses-the-atime (a_device_that_fails_loses_nothing_durable), a-create-leaves-its-directorys-times (a_clone_to_its_own_end_past_a_partial_block_holds), readdir-lists-special-files-as-files (every_cut_of_a_short_sequence), special-files-lose-their-device (a_device_that_fails_loses_nothing_durable), readdir-lists-the-name-it-resumes-after (names_of_every_spelling_fold_and_list), names-are-made-lossy (a_device_that_fails_loses_nothing_durable), statfs-counts-nothing-used (a_clone_to_its_own_end_past_a_partial_block_holds), allocate-sets-the-size-to-its-end (whatever_a_vfs_asks_reads_back_as_a_model_says), the-tail-is-read-from-its-extent and dax-maps-blocks-never-written (a_file_allocated_part_of_a_block_grows), xattrs-refuse-the-largest-value (not caught by the first run: 64000 exactly was rare; the generator now picks it and 64001 alone, and the_roots_attributes_change_as_any_nodes sets both: red), an-xattr-made-replaces-one-there (tests::xattrs_are_made_at_once), a-directory-folds-case-once-reopened (names_of_every_spelling_fold_and_list). mutants.sh --check: 31 checked, 0 broken.

Validation (macOS): cargo nextest run -p fxfs-posix 80/80 (whatever_a_vfs_asks_reads_back_as_a_model_says 40 s at 96 cases, 41.8 s before); -p fxfs 781/781; -p fxdax 305/306 before the FUSE test that asserted the old bug was corrected, then fuse 17/17. Hunts: FXDAX_MODEL_CASES 3 x 700 (5,765 cuts, 389 refused), 300 with FXDAX_MODEL_WORKERS, FXDAX_MODEL_EVERY_CUT=150 (7,170 cuts at 494 barriers), FXDAX_MODEL_FAILURES=400 (102 operations failed): all pass; an earlier 1,500-case hunt found the xattr-replace ENOSPC order after 750 s (oracle widened, above). scripts/upstream-changes.py --check passes. Not run: the GCE workspace and guest cases (DoD 1): fxfs-posix readdir and fxfs tail.rs, dax.rs and store_object_handle.rs reach the guest kernel; guest/rust/fxfs-linux does not build for the host (its linux-rt lang items).

After the merge into worktree-fuse (2026-11-07), the default run found the model wrong, and hunts on the corrected model found one Fxfs bug and one more oracle gap:

5. The model, not Fxfs: an allocation over pages a fill wrote, then a truncate into the first page, Fxfs left the page out of what was allocated, the model in it (seed kept: a_truncate_into_an_allocation_over_written_pages). Zeroing what the page held past the new end writes it anew, copy-on-write, as every write is, which ends its allocated (overwrite) mode; with a tail of zeros (nothing written) or an aligned cut it stays, as the model has it (both tried). Whether Fxfs gets to zero the tail depends also on room (it logs and leaves it, on a full filesystem) and on cuts (its own transaction, after the cut), and the growth after zeroes it: so the last page's allocated state, where its tail was written to, is the one thing the model takes from Fxfs (Data::unsettled, World::settle_tail), every other page held exactly. Why the hunts missed it: it needs an allocation over written pages, then a truncate into one with a non-zero tail, in that order: rare at the default weights.

6. Fxfs: a queued unlink in a directory whose names fold case (or are encrypted) went through replace_child, which timed the directory (mtime, ctime) and the inode (ctime) as the batch applied, after whatever the kernel wrote back since: a setattr of the directory's mtime after the unlink was overwritten. The guest's VFS keeps those times; a queued unlink in a directory whose names are keys already left them (txn_remove_known_child). Found: a hunt, [Casefold root, Create, UnlinkLater, SetAttr mtime]. Blind spot: the model relaxed every time a queued unlink touched ("no earlier"), which hid a time moved after a later set; only unlink_later's callers (the guest) see it, and casefolding isn't reachable from a VFS yet, encryption neither. Class test: the model now holds queued unlinks to changing no time at all, in every sequence; pinned an_unlink_queued_in_a_casefolded_directory_leaves_its_times; mutant a-queued-unlink-times-its-directory red (that test). Structure: two ways of applying a queued unlink (by key; replace_child) became one, txn_remove_known_child, which looks the key up where names aren't keys and refuses a name linking another object (Inconsistent, as check_replaced did); fxfs test test_txn_remove_known_child rewritten to that (a casefolded name removed in another case, times left). Look around: other callers of replace_child time what they change as Linux does (unlink, rename: the VFS calls them synchronously, the host keeping no directory times; the guest times directories itself, and its write_inode writes them after): fine.

7. Oracle: a clone of allocated blocks on a full device says ENOSPC before EOPNOTSUPP (the transaction is reserved first), changing nothing either way: taken as a refusal, the destination held unchanged (pinned a_clone_of_an_allocation_on_a_full_filesystem_is_refused).

Seeding: the model tests build their Config from Config::default(), whose rng_seed is RngSeed::Random, from the OS's entropy, each run, unless PROPTEST_RNG_SEED fixes it: every default run tries 96 new sequences.

Merged with 4497a6bec (TASK-149.39: fallocate grows the file and sets nothing aside): the model's Allocate grows the file (Data::allocate), sets nothing aside, holds blocks to what writes take, and is held to the room Fxfs counts (free blocks and the file's own, `Fxfs::allocate`): one accepted past it, give or take the block a growth's tail zeroing takes, fails. Gone with preallocation: Data.allocated and unsettled, settle_tail, adopt_allocated, clone refusals of allocated blocks, and the mutants the-tail-is-read-from-its-extent, dax-maps-blocks-never-written and allocate-sets-the-size-to-its-end, which the model can't reach any more. New: holds_extents fails any extent written in place and any data record saying it has some, after every data operation (not only when fsck runs); mutants allocate-preallocates (TASK-149.39's bug back), allocate-shrinks-to-its-end, allocate-ignores-the-room. Pinned tests turned to what is true now: an_allocation_grows_a_file_setting_nothing_aside (with the fuzzer's case: a byte allocated at 28672, a write from 28469), an_allocation_over_written_pages_changes_nothing, allocations_on_a_full_filesystem_take_only_the_room (an allocation of the whole device refused). The tail.rs and dax.rs fixes stay, still right for preallocated extents images hold and for DataObjectHandle::allocate's own callers (none of fxdax's): their fxfs unit tests, which call it, keep them (OPEN_QUESTIONS entry rewritten).

Blind spot, why the model never hit MissingOverwriteExtents though it ran fsck: (1) fsck ran only at a sequence's end, after crashes and in cut checks, and the flag is wrong only while no overwrite extent is left: a write had to replace every block an allocation made, and nothing after (a truncate, which drops the flag with the extents, or another allocation) set it right again before the check. (2) The generator made that rare: allocations of 1 to 64 pages or thousands, at offsets drawn apart from the writes', which are 1 to 3 pages, so a write covering all of an allocation came seldom. (3) Most of all: the model's own oracle saw writes take blocks out of overwrite mode (the extents it compared), and I took that for Fxfs's design ("writes end allocated mode") and taught it to the model, without asking what else in Fxfs recorded overwrite mode and whether it stayed true: an oracle fitted to what it observed instead of holding an invariant of it. The model now checks the record's flag and the extents after every data operation, so a write that leaves either fails at once.

Also found while merging: a cut during the replay of a cut left a truncate of a thousand extents to the graveyard's trim, which runs after the mount, the file counting its blocks until then, and the second replay was counted before syncing: it now syncs until nothing waits to be freed first (lets_go), as the first replay does; pinned a_cut_during_a_replay_counts_blocks_once_trimmed (the default run's seed). Held to the first replay's full orphan check, such replays hold extents Fxfs doesn't know: TASK-149.41, with the reproduction.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The fxfs-posix model test holds every node's attributes as Linux's VFS sees them (mode, owner, group, rdev, times, xattrs, st_blocks), from the root's defaults, across cuts, remounts, tiny and failing devices, with new operations for special files, setattr, fallocate, readdirs going on after a name, casefolding, bad names, xattr modes and limits, and statfs. It found three bugs, each fixed in a commit of its own with a test red before: readdir listed FIFOs, sockets and devices as regular files (now taken from each file's record, one node_type; host FUSE listing fixed too); fallocate of part of a block then any growth failed EIO (the tail zeroing now reads through the one read path); a DAX window would map blocks an allocation left unwritten (extents_in now leaves them out). 15 mutants, all red. Verified on macOS (fxfs-posix 80/80, fxfs 781/781, FUSE tests, hunts of 2,100 + 300 + 150 + 400 sequences); the GCE run is still to do.
<!-- SECTION:FINAL_SUMMARY:END -->
