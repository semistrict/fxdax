---
id: TASK-17
title: 'fxdax serves FUSE itself, over buffers it only copies'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 01:02'
updated_date: '2026-10-10 14:44'
labels: []
dependencies:
  - TASK-16
type: feature
ordinal: 17000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every VMM has to write a FUSE adapter (~600 lines for libkrun) to use fxdax, and zero-copy reads and writes took IoSlice/IoSliceMut, which a VMM can only build over guest memory by forming Rust references to memory the guest may change concurrently, which is unsound. The user chose for fxdax to decode virtio-fs FUSE requests itself, given the request and reply as buffers it only ever copies into or out of (volatile slices), so that a VMM only converts a descriptor chain into those buffers. Do not commit (user instruction, as for TASK-16).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxdax has a FUSE server that takes a request as readable buffers and a reply as writable buffers, and returns the reply length; it implements the requests the libkrun adapter did, and replies ENOSYS to the rest
- [x] #2 Reads and writes, of the server and of Filesystem, copy into and out of volatile slices without forming references to their memory; the kernel port takes them as addresses and lengths
- [x] #3 Tests drive the server with encoded FUSE requests: init negotiation, entries, I/O, directories, attributes, DAX mappings, errors and malformed requests
- [x] #4 libkrun serves fxdax devices with the fxdax server, converting descriptor chains into volatile slices; its FxdaxFs adapter is gone; its guest tests pass on macOS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS
- [ ] #5 Ported code names its upstream source
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. memory.rs: VolatileSlice (readable) and VolatileSliceMut (writable), from slices or unsafely from raw parts; Send + Sync; only copied.
2. Kernel: async stream reads/writes and VMO vectored I/O take host buffers (address, length), copied with user copies.
3. Filesystem::read_vectored/write_vectored take volatile slices.
4. fuse::abi: the FUSE structs and constants used, ported from libkrun fuse.rs (crosvm). fuse::Server: new(Filesystem), handle(request, reply) -> reply length; reader/writer cursors over slices; INIT negotiation (max write, DAX map alignment when a window exists, readdirplus, atomic O_TRUNC, KILLPRIV_V2); handlers ported from libkrun server.rs and the FxdaxFs adapter.
5. tests/fuse.rs: a test client encoding requests and decoding replies.
6. libkrun: fxdax devices use fxdax::fuse::Server through descriptor-chain slices; FxdaxFs removed; an fxdax root with implicit init is unsupported.
7. Verify on macOS (suites, e2e) and Lima (build, suites).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax: memory.rs VolatileSlice/VolatileSliceMut; kernel async I/O over user_copy::HostBuffer; fuse/abi.rs (ported from libkrun fuse.rs) and fuse/mod.rs Server (ported from libkrun server.rs + FxdaxFs). INIT offers ASYNC_READ, PARALLEL_DIROPS, BIG_WRITES, AUTO_INVAL_DATA, ASYNC_DIO, ATOMIC_O_TRUNC, DO_READDIRPLUS, READDIRPLUS_AUTO, MAX_PAGES, HANDLE_KILLPRIV_V2, INIT_EXT (no security contexts/submounts/idmap). Malformed bodies reply EINVAL; undecodable header or reply overflow are errors. tests/fuse.rs: 12 tests with an independent hand-encoded client. libkrun: FxdaxServer hands chains as volatile slices (marks writable descriptors dirty on macOS), FsServer::handle_chain; FxdaxFs removed (-600 lines); fxdax devices with virtual entries (a root with implicit init) are refused.

Validation: fxdax 57 tests (12 FUSE protocol tests) on macOS; full suites macOS 1800, Lima 1803; libkrun virtiofs-fxdax and -nodax pass on macOS (HVF) and Linux (KVM in Lima) with libkrun handing descriptor chains to fxdax::fuse::Server. Not committed, at the user's instruction.

Pre-commit review fixes: SETATTR times before the epoch, too late, or with nanoseconds >= 1e9 fail with EINVAL instead of panicking the VMM's worker (test setattr_rejects_invalid_times); READDIRPLUS leaves out entries removed since opendir and keeps entries already looked up when a later lookup fails, looking up in the handle's directory (DirEntries::directory) rather than the request's node (test readdirplus_skips_removed_entries); entry names are validated (not empty, ., .., or containing /, at most 255 bytes) and request bodies must end with their last name (test names_are_validated); VolatileSliceMut is written only through &mut and is no longer Sync, so Server::handle and Filesystem::read_vectored take &mut slices. Deferred: TASK-24.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax decodes virtio-fs FUSE requests itself: fxdax::fuse::Server::handle takes a descriptor chain as volatile slices of guest memory, which it only copies into and out of, and returns the reply length. File data is copied straight between guest memory and file pages through the kernel port, which takes buffers as addresses and lengths. libkrun now hands chains to the server and its 600-line FxdaxFs adapter is gone; fxdax roots with libkrun's implicit init are refused. Verified by hand-encoded protocol tests, the full suites on macOS and Linux, and the libkrun guest tests on both.
<!-- SECTION:FINAL_SUMMARY:END -->
