---
id: TASK-149.39
title: Fuzz the FUSE server with malformed and random requests
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 05:20'
updated_date: '2026-11-07 11:17'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 662000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax::fuse::Server (TASK-222) decodes requests from the kernel, which a FUSE server trusts only so far: a bug in decoding could panic the mount's worker threads or answer a request wrongly. Its tests are examples. A proptest of random requests, valid and malformed (lengths, truncated bodies, names without NULs, unknown opcodes, node IDs and handles never given or forgotten, forgets of more lookups than given, readdir offsets past the end, xattr sizes), sequences of them against one server, checks it never panics, answers each request with a well-formed reply or none as the protocol says, and keeps its tables consistent (lookups counted as the kernel would count them).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A seeded, shrinking proptest sends random sequences of valid and malformed requests to a Server and checks no panic, a well-formed reply for every request that has one, and the node and directory tables as a model of the kernel's lookups says
- [x] #2 It is shown to find a mutant of the server's decoding and of its lookup counting, red
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/fxdax/src/fuse/server/fuzz_tests.rs: a proptest of sequences of requests to one Server over a memory store: valid ones (every opcode served, names from a pool, node IDs held, stale, never given, the root, handles open, released, never given, readdir offsets past the end, xattr sizes, forgets of more than held) and malformed ones (header length short or past the request, bodies truncated, names without NUL, trailing bytes, unknown opcodes).
2. Checks: no panic; a reply exactly when the protocol has one, its header's length and unique right, an errno or a payload of the opcode's shape; malformed ones EINVAL, unknown ENOSYS, stale ESTALE, bad handles EBADF; the server's lookup and directory tables equal to a model of the kernel's lookups and handles; at the end DESTROY, the store closed and checked.
3. Mutants of the decoding and of lookup counting shown red.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: crates/fxdax/src/fuse/server/fuzz_tests.rs, a child of the server's module (to read its tables): proptest sequences (INIT, then 1-119) of valid requests of every served opcode (names of a pool, invalid ones, random bytes; node IDs held, forgotten, never given, 0, the root; directory handles open, released, never given; offsets and sizes at u64/u32 edges; modes, times out of range; forgets of more than held, batches) and malformed ones (header length short of a header or past the request, a message cut short, a name's NUL dropped, bytes after the last name, a write's or xattr's size past its data, random bodies and unknown opcodes). Checks: no panic; a reply exactly for opcodes that have one; header length and unique; errors named errnos without payload; payloads of the opcode's shape; entries' node IDs and inode numbers swapped as the server swaps them (root/1); readdir dirents whole, padded, offsets increasing, names valid, READDIRPLUS entries but . and ..; malformed EINVAL, unknown ENOSYS, a node not held ESTALE (or EINVAL/ENOTSUP the server checks first), a handle not open EBADF; the lookup table (counts, each node its ID's object) and open-directory table equal to the model after every request; DESTROY, close, fsck at the end. FXDAX_FUSE_FUZZ_CASES (256 default, ~5 s on macOS). filesystem::valid_name pub(crate) for the dirent check.

Bug found (fixed, own commit): fallocate then a write across the allocation left fsck's MissingOverwriteExtents. Found: the fuzzer's third case (MKNOD, FALLOCATE 1 byte at 28672, WRITE at 28469). Cause: fxfs-posix's allocate used Fxfs's preallocation (DataObjectHandle::allocate), overwrite extents and the attribute's has_overwrite_extents flag; fxdax writes copy-on-write (write_many), replacing the overwrite extent, the flag left set. The guest's fallocate never preallocates (fxfs_data_fallocate: grows, checks room), so only the host made them. Blind spot: no test wrote over an allocation (tests/filesystem.rs allocate only checked blocks; fxfs-posix's model test has no fallocate, TASK-149.36 adds it). Test: filesystem writes_across_an_allocation_past_the_end, red before the fix (lengths from 204 on, the write reaching the allocated block). Fix: fxfs-posix allocate checks room (usage, not statfs, which drains unlinks against Allocate's row) and grows the file by setattr, as the guest; the allocate test now expects no blocks and ENOSPC past room. Looking around: DataObjectHandle::allocate has no other caller in fxdax (journal/superblocks use preallocate_range); its module doc now says so. Structure: one fallocate semantics for host and guest makes the class impossible from fxfs-posix; Fxfs's preallocation with fxdax's copy-on-write writes stays inconsistent (the flag has two writers and a write that replaces extents doesn't clear it), unreachable now: recorded in OPEN_QUESTIONS as what undoing would take.

Mutants (red): decoding, handle()'s body taken whole when the header's length is past it (LongHeader MKNOD: a 128-byte payload where EINVAL); lookup counting, entry() setting lookups to 1 rather than adding (Mkdir then Lookup: server {3:1}, kernel {3:2}).

Landing: fuse::server::fuzz_tests::random_requests_are_answered_as_the_protocol_says passed on GCE run-20261107T093421Z (workspace 2312/2312) and in CI 42356882950.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A proptest fuzzer of fxdax::fuse::Server (fuse/server/fuzz_tests.rs): valid and malformed request sequences, each reply checked for shape and errno, the lookup and directory tables held to a model of the kernel's, fsck at the end. It found fallocate's preallocated overwrite extents corrupting stores once written (fixed in fxfs-posix, 4497a6bec: fallocate grows the file, as the guest's). Mutants of decoding and lookup counting red. Verified: 5,000 cases on macOS, Lima, CI 42356882950 and GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
