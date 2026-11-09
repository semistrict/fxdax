SQLite on sqlite_extents (fxdax image, pd-balanced boot disk) against stock SQLite on ext4 on the same disk (TASK-298). Two runs.

Machine: c3-standard-8, in us-east4-a.

Findings (both runs agree within 5%):

- Single-row commits: extents 337/s (p50 2.9 ms) against 365/s for rollback FULL; WAL FULL is 3x faster (1,082/s, p50 0.96 ms), one fsync a commit.
- Single-row updates: extents 211/s (p99 13 ms) against 363/s rollback FULL and 1,060/s WAL FULL.
- 500-update transactions: extents 3/s (344 ms) against 13/s rollback FULL and 22/s WAL FULL.
- Point reads: extents 987/s against 236,000/s and 466,000/s: about 1 ms a page read below SQLite's cache, which is 2 MiB of an 11 MiB database. Scans are 250x slower likewise.
- Space: 17.2 MB (image and its bucket) against 11.3 MB.

The reads dominate: the volume keeps no cache of its own, and every page SQLite's cache misses goes to the image, whose uploaded data is read through its bucket's fetcher, about 1 ms a 4 KiB block. Extents are immutable, so a block cache in the volume needs no invalidation (TASK-300).
