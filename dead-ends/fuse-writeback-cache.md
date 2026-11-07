# FUSE_WRITEBACK_CACHE on fxdax's FUSE server

**Tried:** 2026-10-10, TASK-25.9 step B, measured on GCE (gce-fs-bench run T2151Z).
**Idea:** Only the guest changes an fxdax filesystem, so it can cache all it can, writes included.
**Result:** Unlinks 3,530 -> 2,560/s, creates without DAX 2,193 -> 1,973, sequential writes without
DAX 58 -> 43 MiB/s. Random writes without DAX 4,560 -> 5,469 and the build 2.5% quicker.
**Why it's dead:** It cost more than it saved. With it, closing a written file writes it back and
sends its times (fs/fuse/file.c, fuse_flush). The FUSE server itself has since gone (fda9d7d2).
**Reopen if:** A FUSE path comes back. Guest Fxfs doesn't use one.

A FUSE path came back on 2026-11-07: the host's mounts of stores (`fxdax::fuse`, `fxdax-mount`,
TASK-222), a server descended from the one measured here, over `/dev/fuse` rather than virtio-fs.
It doesn't offer the writeback cache either, unmeasured on that path: measure it there before
turning it on, as the kernel's flush on close costs the same.
