TASK-25.9 step A: fxdax's FUSE server caches all the guest can as the only writer: no AUTO_INVAL_DATA or ATOMIC_O_TRUNC, CACHE_SYMLINKS, OPEN and FLUSH refused (and so RELEASE not sent), negative entries, cached directory entries, FOPEN_NOFLUSH; no writeback cache.

Iterations: 3. Cases: bench-fs-fxdax-dax bench-fs-fxdax-nodax. Workloads: all.

Finding, against 2026-10-10T2124Z: mmap random reads without DAX 9,632 -> 1.20 million a second
(ext4 1.20 million), the guest keeping its cached pages; creates 1,545 -> 2,193 a second with DAX
and 1,547 -> 2,193 without, without FLUSH and RELEASE. The rest is unchanged.
