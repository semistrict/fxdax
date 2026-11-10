SQLite on sqlite_extents through the extent daemon (reads copied from its window; and SQLite's mmap reading pages in place) against the in-process image and stock SQLite on ext4, same pd-balanced disk (TASK-300). Two runs.

Machine: c3-standard-8, in us-east4-a.

Findings (two runs, within 5% of each other):

- Point reads: daemon 255,000-266,000/s, daemon-mmap 268,000-277,000/s, against 936/s for the in-process image, 236,000/s for rollback FULL and 460,000/s for WAL. From the daemon's cache through the window, reads are 280x faster than through the image, and past stock SQLite in rollback mode.
- Scans: daemon 20M rows/s, daemon-mmap 25M rows/s (pages read in place: 158,133), against 17M for stock SQLite.
- 500-update transactions: daemon 16/s against 3/s through the image, 13/s rollback FULL, 22/s WAL FULL.
- Single-row commits got slower: 130/s (p50 7.5 ms) through the daemon against 340/s through the image and 327-394/s rollback FULL. Each commit is several round trips to the daemon (the extent's write, the durable commit, the frees, the window's MAP_MANY), and the daemon's durable commit; not yet profiled (TASK-301).
- Bytes on disk for daemon configs count the store's directory, which holds the host's part cache of what was uploaded, and the bucket: not comparable to the others.

Built with the extent store's audit, which recounts its state after each critical section (~8% of a one-row commit's CPU, TASK-301): scripts/lib/example-linux-host.sh built the example across the workspace, whose dev-dependencies turned it on, until 70a12ea15; a release build now refuses it (TASK-303).

Built also with fxdax's failpoints (its dev-dependency on itself, bee7356a6, 2026-11-01), each `fail_point!` a lookup in the fail crate's registry, and extent_store's `testing` (3afa08d64, 2026-11-08), each lock shared beyond one store taking a process-wide one too: the workspace's dev-dependencies turned them on. About 1-3% of a small request's time on extentd-mapbench (ubu1-mapbench-failpoints-2026-11-10); a release build now refuses them (TASK-320).
