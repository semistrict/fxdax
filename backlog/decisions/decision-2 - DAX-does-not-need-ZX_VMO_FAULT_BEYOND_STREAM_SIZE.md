---
id: decision-2
title: DAX does not need ZX_VMO_FAULT_BEYOND_STREAM_SIZE
date: '2026-09-28 04:46'
status: accepted
---
## Context

The kernel port lacks `ZX_VMO_FAULT_BEYOND_STREAM_SIZE`, which upstream uses so that mappings of a file fault past its stream size instead of seeing zero pages (mmap's `SIGBUS`). TASK-7 asked whether DAX mappings need it.

Linux's FUSE client (`fs/fuse/dax.c`) maps files in 2 MiB ranges, which extend past a file's end, and bounds accesses by `i_size` itself:

- it reports holes past `i_size` without asking the server;
- a guest mmap past the last page gets `SIGBUS` from the guest kernel;
- a write that extends the file becomes `FUSE_WRITE`, not a store through the window.

## Decision

DAX does not need `ZX_VMO_FAULT_BEYOND_STREAM_SIZE`. The window maps files' `UNBOUNDED` VMOs, and pages past the stream size read as zeros.

`crates/fxdax/src/vfs/tests.rs` `dax_past_the_end_of_the_file` tests this:

- the guest reads zeros past the end;
- a store past the end in the last page never reaches the file, because Fxfs writes back only what is before the end and zeroes the rest of the last block;
- the size is unchanged after fsync and remount.

`crates/fxdax/src/dax.rs` documents this behavior.

## Consequences

- Pages past EOF can be reached only when the guest's `i_size` is stale. The guest then sees zeros, and its stores there are dropped, as with a shared mmap of a file truncated elsewhere.
- The kernel port keeps omitting the flag and its stream test `FaultBeyondStreamSizeResizeDownRace`.
