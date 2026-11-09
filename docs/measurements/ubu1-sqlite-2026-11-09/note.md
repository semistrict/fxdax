# SQLite on sqlite_extents against stock SQLite, on ubu1 (TASK-301)

Branch task-301 at e0a06e098 (the code measured is 73427fa6e's, unchanged since but for tests). Host: ubu1 (host.txt), databases on its /home, a ZFS dataset, built `-p sqlite_extents` alone, without extent_store's audit (TASK-303).

`sqlite-bench` five runs (sqlite-bench.txt), means against WAL FULL:

| workload | daemon | daemon-mmap | rollback FULL | WAL FULL |
|---|---|---|---|---|
| 1-row commits/s | 1,243 (90%) | 1,195 (86%) | 415 (30%) | 1,385 |
| 1-row updates/s | 1,019 (77%) | 989 (75%) | 423 (32%) | 1,318 |
| 500-update transactions/s | 38 (57%) | 37 (55%) | 47 (69%) | 67 |
| point reads/s | 283,490 (111%) | 482,246 (189%) | 98,737 | 254,728 |
| scans, rows/s | 10.5M (101%) | 30.7M (295%) | 10.4M | 10.4M |

`sqlite-commit-profile` (commit-profile.txt): on ZFS an overwrite syncs no faster than an append (0.64 ms either way), unlike GCE's pd-balanced ext4 (0.32 ms against 1.0 ms), so recycled logs gain nothing here. A one-row commit through the daemon: its write and commit, one request, 0.86-0.91 ms, of which the log's append and sync 0.76-0.81 ms.

Bytes on disk are not comparable: ZFS's `du` counts compressed blocks, and the daemon's directories hold its cache of what was uploaded.
