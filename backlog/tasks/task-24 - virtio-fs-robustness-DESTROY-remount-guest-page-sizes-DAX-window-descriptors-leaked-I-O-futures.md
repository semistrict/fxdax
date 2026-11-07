---
id: TASK-24
title: >-
  Image device: an I/O future leaked with mem::forget lets its buffer be reused
  while the kernel writes it
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 14:41'
updated_date: '2026-11-07 05:30'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in the pre-commit review of TASK-17/18. Of its four findings three went with virtio-fs: the FUSE server (crates/fxdax/src/fuse, deleted in 610923b34) and libkrun's virtio-fs device (vendor/libkrun, removed in 5ba394977, TASK-40), so a remount after DESTROY, INIT's max_pages and DAX-window descriptors in replies no longer exist to fix.

Left: the image device's I/O (crates/fxdax/src/storage/io.rs:43-52, Io::run): an I/O future leaked with mem::forget lets its buffer be reused while the kernel still writes it. Io::run's unsafe contract says not to leak it, but ImageDevice implements storage_device's safe Device::read_with_opts over it (crates/fxdax/src/storage/image.rs:121, 149-163). Consider owning bounce buffers or a different contract.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The image device's buffers stay valid even if an I/O future is leaked, or the decision not to is recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Confirm it is reachable from safe code.
2. A test that fails first: a test-only hold of an operation in Io, a read forgotten in flight, its memory reused, the operation let go.
3. Fix: Io::run takes ownership of the buffer and the file, which its completion holds until the host is done; the device copies between its buffer and the caller's. No forget, no blocking drop.
4. fxdax storage tests on macOS (thread pool) and on Linux (io_uring, Docker with seccomp unconfined); fxdax suite.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Reachable
Yes. ImageDevice implements storage_device's safe Device::read_with_opts and write_with_opts over Io::run (storage/image.rs), and their futures borrow the caller's buffer. Safe code may poll one, so the operation is submitted, then mem::forget it: that ends the borrow while the I/O thread or the kernel still reads or writes the buffer. Io::run's unsafe contract said not to leak the future, and InFlight's Drop waited for the operation, but a leaked future is never dropped. Its fd was the caller's too: a file closed meanwhile could have its descriptor reused by another open, so a pwrite landed in a different file.

## Test that fails first
Io gained a test-only hold: the next operation's submission waits, on a thread of its own, until the test lets it go, so the test can forget an operation guaranteed to be in flight on either engine. On main's API a temporary test read into a Vec, forgot the future, reused the Vec (filled 0xaa), let the operation go and waited for it: "a leaked read wrote memory given back". Red.

## Fix
storage/io.rs: Op::Read { len, offset } and Op::Write { data: Aligned, offset }, and an IoFile (Arc<dyn AsRawFd>). Io::run is safe: the operation's Completion owns the buffer and the file (State::InFlight) until the engine completes it, then keeps the buffer for the future (State::Done) and lets the file go. InFlight yields Done { result, buffer }. Dropped or leaked, it leaves both to the completion, which the engine frees once the host is done; Drop no longer blocks, so the future is cancel-safe. The engines take a Raw { write, addr, len, offset } into memory the completion owns. storage/image.rs: read_file reads into an owned Aligned, which a Packed part's unpacking takes as it is and a Data part's read copies into the caller's buffer; write_file copies the caller's data into an owned buffer. One copy per read and write: this device is an in-process image's (imports, host tools, tests), while guests' I/O goes through extentd.

## Tests
storage::io::tests::a_leaked_read_writes_only_what_it_owns: a read held in flight and forgotten keeps the file open; let go and waited for, through a new future of the same completion, it read into its own buffer and let the file go. storage::io::tests::operations_give_their_buffers_back. fxdax storage:: 18 of 18 on macOS (thread pool) and on Linux in Docker (io_uring, with --security-opt seccomp=unconfined, as the default profile refuses io_uring with EPERM, on main too).

## Structure
Io::run's safety rested on a contract (don't leak) that its callers' safe API couldn't keep. Ownership puts it in the types, as io_uring's owned-buffer APIs do. Looked around: Io::run is the only unsafe I/O entry point in fxdax's storage, and no other unsafe fn takes a borrowed buffer across an await.

Verification: cargo nextest run -p fxdax on macOS 289 of 289; storage:: on Linux in Docker (io_uring) 18 of 18; clippy and rustfmt clean on the two files. The workspace on GCE not run (DoD #1 open).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Reachable: ImageDevice's safe Device reads and writes lent the caller's buffer, and its file, to Io::run, whose unsafe contract (don't leak the future) safe code could break, leaving the host writing memory the caller had back. Shown red first with a test-only hold of an operation in flight, forgotten, its memory reused. Now Io::run is safe: each operation owns its buffer and file until the host is done, gives the buffer back as it completes, and no longer blocks when dropped. The device copies between that buffer and the caller's. fxdax passes on macOS (thread pool) and Linux (io_uring).
<!-- SECTION:FINAL_SUMMARY:END -->
