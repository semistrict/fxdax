---
id: TASK-149.17
title: 'A compromised VM process, checked against invariants across tenants'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 16:53'
updated_date: '2026-11-06 09:51'
labels: []
dependencies:
  - TASK-77
parent_task_id: TASK-149
priority: high
type: task
ordinal: 534000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-77 fuzzes the wire protocol and asks that nothing panics and the daemon serves on. Three audit findings pass that: they need stating what must stay true. Two tenants, one hostile, each request sequence followed by checks of the other.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Random request sequences, on one connection and concurrent ones, from one tenant of two
- [x] #2 The daemon memory stays within its bound for the connection
- [x] #3 The other tenant commits, reads its own data, and no path stays held
- [x] #4 A store used only as the protocol allows keeps committing
- [x] #5 A bug of each kind, put back, fails it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A proptest in extentd (tests/hostile.rs, a child of extentd's tests): two granted tenants, mallory (hostile) and alice (correct), and the default tenant's bob, on one daemon.
2. Each case: 1-3 of mallory's connections at once, each saying anything (any kind, flags, words naming its own, alice's and bob's branches and extents, paths escaping its directory, a symlink into alice's, forks to others' branches, restores, transfer areas good and bad, PRIVATE and UNMAP storms, forged, early and missing ACKs), then hanging up mid-request or after, with broken framing; one may flood unread READ replies past its room. Alice meanwhile runs a random sequence of the protocol's requests, writes, commits, reads, frees, MAPs, stores and loads through a copy-on-write window in this process, each result checked against a model.
3. Invariants after each: alice's results all as modelled, her commits succeed, her closed store unchanged; bob served; nothing mallory got names alice's or bob's branch, holds their bytes, or maps an arena not its tenant's or the public one, and its memfds hold no byte of theirs; the daemon's heap within the sum of the rooms of mallory's connections (a counting global allocator, aborting past it, as fxfs-posix's corrupt tests do); a flood past the room is let go; every connection ends, every path mallory named opens again, no panic.
4. Cases from FXDAX_HOSTILE_CASES; CI's workspace part runs the cheap default; a Hunt job runs many with seeds of its own.
5. Mutants for AC5: each kind put back (window bound TASK-150.3, branch named TASK-150.4, path held TASK-150.5, commits wedged TASK-150.7, an arena or extent of another tenant's), each seen red.
6. Fix what it finds, each with a pinned test; macOS FXDAX_TEST_PARTS="workspace shuttle" and Linux in Docker; a 30-minute run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: built as a proptest in extentd, crates/fxdax/src/extentd/tests/hostile.rs (a child of extentd's tests): a_compromised_vm_process_harms_no_tenant_beside_it. Three tenants on one daemon: mallory (granted, hostile, 1-3 connections at once), alice (granted, correct: one connection through a copy-on-write window in this process, each result against a model; her loads and stores only where the protocol allows, a refused store restricting its page until it changes, which stops a real VM), bob (default tenant, waiting). Invariants after each case: alice as modelled and committing; her closed store unchanged though mallory names it (by .., through a symlink in mallory's directory, by path); nothing of alice's or bob's in mallory's replies, stage operations or memory (both memfds scanned whole for their marks); no foreign arena mapped; the process heap within the sum of mallory's connections' rooms (a counting #[global_allocator] aborting past it, after fxfs-posix's corrupt tests); floods of unread replies let go; every mallory connection ends, every store it named reopens (not EBUSY), every store it wrote reopens and commits, no point taken of a branch named, alice's and the root directories unchanged; no panic, no crash. Cheap: 48 cases (FXDAX_HOSTILE_CASES) in CI's workspace part; Hunt job 'extentd' runs 4 shards of 1,920 from run-number seeds. Chose proptest over cargo-fuzz: the invariants are of a stateful three-tenant daemon with threads, and proptest's replayable seeds and cases fit the Hunt job; nothing new to license (memchr, already in the workspace, MIT/Unlicense, as a dev-dependency).

Found (each with a pinned test, red before, and a mutant in crates/fxdax/mutants):
1. HIGH, daemon crash for every tenant: a VM process ftruncate()s the memfd of its tenant's arena, which the daemon hands it read-write, and the daemon's next access to a page past the new end SIGBUSes. Firecracker's vmm-thread seccomp filter allows ftruncate unconditionally ('Used for snapshotting'). Found by the harness at case 114 of seed 13 (ABORT SIG 7). Fixed in physmem: arenas' memfds made with MFD_ALLOW_SEALING and sealed F_SEAL_SHRINK; resize only grows, tolerating a VM process having grown the object. Tests: vm_processes_shrinking_their_memory_crash_nothing (SIGBUS before), physmem arenas_objects_are_sealed_and_their_owners. Mutant arenas-shrink.
2. MEDIUM, cross-tenant write, needs /proc in the jail: the default arena, every VM process's read-only public memory (public bases' pages and the global zeros page), is a memfd, mode 0777, so a process of another user can reopen /proc/self/fd/N O_RDWR and write every tenant's zeros, or shrink it. Now 0600 (and sealed against shrinking as in 1). Same-uid processes (the default tenant's) can still; they are the daemon's user. Pinned by the mode assertion in vm_processes_shrinking_their_memory_crash_nothing and arenas_objects_are_sealed_and_their_owners: an in-process test can't show another uid's open.
3. MEDIUM, memory past the connection's bound: ExtentStore::read and write copied each request's whole length into the heap (and extent_store's write copied it again), 8 requests running per connection, uncounted by Peer::room, a transfer area's not counted at all. One connection within the protocol held 481 MiB of reads, 512 of reads into its transfer area, 1,024 of writes from it, 640 of writes, against a room of 272 MiB. Fixed: copies of 1 MiB pieces (ExtentStore::PIECE; extent_store Store::write_with; memory::gather/scatter). Now 8 MiB through the area. Test: what_requests_hold_is_within_the_room. Mutant copies-of-whole-requests.

4. MEDIUM, memory past the bound, and every commit's footer: a VM process's root (COMMIT_ROOT, WRITE_ROOT) could be 64 MiB, where the extent device's own MAX_ROOT is 4 KiB; the store kept it, wrote it with its commits, and each ROOT copied it into a reply the room didn't count: 16 at once held 768 MiB. Found looking around 3. Fixed at the wire: wire::MAX_ROOT 4096, EINVAL past it. Pinned in what_requests_hold_is_within_the_room. Mutant roots-of-any-size.

Blind spots: TASK-77's fuzz and whatever_a_vm_process_says_harms_no_other checked that nothing panicked and another store served on, never what the VM process did outside the socket (the memory it is given is an interface too: 1, 2) nor what the daemon held (3, 4: nothing measured the heap; Peer::room bounds admission, not what serving holds). Class tests: the hostile harness abuses the memory it is given and counts the process's heap; what_requests_hold_is_within_the_room measures each request kind at the protocol's limits.

Structure: 1 and 2 have one owner already (physmem's create_shared_memory makes every object handed out; sealed there now, so no arena can be made otherwise); the transfer area, the one memfd the daemon takes, was already checked for its seal. 3: each request's own copy is bounded in the two functions all reads and writes go through. 4: the protocol's bounds live in the codec (MAX_WINDOW, MAX_BUILD, MAX_ROOT now). Not fixed, filed: TASK-149.34 (page-ins in flight per connection, bounded by nothing it keeps within), TASK-149.35 (a tenant's in-flight data bounded per connection, not per tenant: 1,024 connections times a 272 MiB room; a design question for the user).

AC5, mutants (crates/fxdax/mutants, run by crates/fxdax/mutants.sh, --check in scripts/test.sh's scripts part): windows-of-any-size (TASK-150.3) and no-room-checked (79.16), memory; a-second-open-holds-its-path (150.5), path held; tenants-name-branches (150.4), tenants-name-any-store (43.4), tenants-given-the-default-memory (79.17), another tenant's; forks-take-branches-claimed (79.82: a fork to bob's branch fenced bob, EROFS), commits wedged; arenas-shrink, copies-of-whole-requests, roots-of-any-size, this task's. All 10 killed at 96 cases (Docker, OrbStack, aarch64). TASK-150.7's own mechanism can't be put back: writes-of-part-of-a-block (check_io's alignment removed) wasn't killed, and is equivalent for commits now: the writable DAX window that left open.len unaligned is gone (doc-13), and the open segment is read through the page cache, so a seal of unaligned pieces succeeds; dropped, forks-take-branches-claimed standing for the kind.

Overlap with other agents: extentd/codec.rs and wire.rs (MAX_ROOT, beside TASK-141.3's wire refusals); extentd/server.rs only QUEUED_ROOM made pub(super); extents.rs read/write and one added extent_store method, Store::write_with (TASK-165.x work in extent_store); tests.rs gains one mod line, the tests living in extentd/tests/hostile.rs.

Runs at 4abc16ade and after (merged with main at 60c862a2e): Linux in Docker (OrbStack, aarch64, privileged): the hostile test 6,000 cases seed 103 (23.6 min) and 9,000 cases seed 104 (48.6 min), all passing (seed 104: 243,545 of mallory's requests served, 2,412 restores, 8,480 of its stores committed at the end, 432 floods let go); an earlier run of seed 102 passed 14,160 cases in 2 h before its output was cut; one of seed 101 found the harness's own limit at about case 4,096 (bob's store full, fixed in its last commit). nextest -p fxdax -p extent_store -p physmem: 771 of 777, the hostile test's then too-rare restore check (fixed since) and 5 of the container's: builds_are_stamped_with_their_commit (no git), grants' three (no sudo), seccomp's (this worktree's vendor/firecracker behind main's; passes at e64c4b283). Shuttle: extent_store 62/62 and 7/7, physmem 6/6, fxdax 59/59. Mutants: 10 of 10 killed. macOS: FXDAX_TEST_PARTS="workspace shuttle" scripts/test.sh, workspace 2,120 of 2,120 and every Shuttle part. scripts/upstream-changes.py --check passes. Not run: the whole Linux workspace (Lima or GCE).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A proptest of a compromised VM process beside other tenants (extentd/tests/hostile.rs): mallory's 1-3 connections saying anything and abusing the memory they are given, alice's correct VM process checked against a model, bob's store waiting; invariants after each case on what mallory gets, what the daemon's heap holds, what stays held, and alice's and bob's stores. Found and fixed: a VM process shrinking its arena's memfd crashed extentd (SIGBUS, every tenant; physmem seals arenas against shrinking, 0600); the public arena reopenable to write via /proc; requests holding copies of their whole lengths past the connection's room (1 MiB pieces); 64 MiB roots (MAX_ROOT 4 KiB at the wire). 10 hand mutants of each kind killed; CI runs 48 cases, Hunt 4 x 1,920. Verified: macOS workspace and Shuttle; Linux (Docker) fxdax, extent_store and physmem, Shuttle, and runs of 6,000 and 9,000 cases (48.6 min). Follow-ups: TASK-149.34, TASK-149.35.
<!-- SECTION:FINAL_SUMMARY:END -->
