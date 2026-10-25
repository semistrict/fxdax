Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 184 (183–184) | 836 (753–843) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,748 (2,559–2,843) | 2,621 (2,619–2,677) | 1.05× |
| sqlite-mmap-scan | MiB/s | 3,400 (3,303–3,459) | 3,464 (3,353–3,537) | 0.98× |
| sqlite-mmap-scan-warm | MiB/s | 3,806 (3,732–3,846) | 3,842 (3,747–3,845) | 0.99× |
| sqlite-delete-full-batch1 | txn/s | 176 (166–192) | 264 (262–270) | 0.66× |
| sqlite-delete-full-batch100 | txn/s | 57.4 (55.9–58.4) | 55.1 (54.9–55.3) | 1.04× |
| sqlite-wal-full-batch1 | txn/s | 355 (338–390) | 617 (605–618) | 0.58× |
| sqlite-wal-full-batch1-checkpoint | ms | 14.1 (14.0–15.0) | 20.6 (19.9–20.9) | 1.46× |
| sqlite-wal-full-batch100 | txn/s | 114 (112–119) | 153 (152–154) | 0.74× |
| sqlite-wal-full-batch100-checkpoint | ms | 62.4 (61.8–65.6) | 68.1 (62.2–87.4) | 1.09× |
| sqlite-wal-normal-batch1 | txn/s | 15,043 (14,143–15,056) | 22,743 (22,297–23,371) | 0.66× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.4 (17.0–18.7) | 36.6 (15.5–94.9) | **2.10×** |
| sqlite-wal-normal-batch100 | txn/s | 270 (269–271) | 431 (359–442) | 0.63× |
| sqlite-wal-normal-batch100-checkpoint | ms | 379 (372–389) | 253 (87.1–283) | 0.67× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.42 median (-4.88–-4.40)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 176 median (166–192)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,740 median (5,586–5,841)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.70)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 355 median (338–390)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,421 median (11,188–11,911)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 15,043 median (14,143–15,056)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,047 median (26,913–27,139)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -1.81 median (-4.33–-1.44)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 264 median (262–270)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,509 median (5,486–5,526)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 617 median (605–618)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 15,339 median (15,160–15,435)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,743 median (22,297–23,371)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 43,145 median (35,866–44,239)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 72 | B | $0.000029 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 317 | A | $0.001585 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 398 | | $0.001649 |

Uploaded 2.1 GiB (segments 1.6 GiB, extents 479.5 MiB, checkpoints 8.1 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,788.0 µs |
| read | 3,977 | 16.2 µs |
| write | 13,445 | 1,195.2 µs |
| free | 596 | 167.8 µs |
| map | 112,376 | 125.7 µs |
| unmap | 17,974 | 30.8 µs |
| commit | 22 | 1,892.4 µs |
| root | 1 | 1.0 µs |
| info | 4 | 1.2 µs |
| window | 1 | 23.5 µs |
| transfer | 1 | 9.5 µs |
| free_many | 1,351 | 276.6 µs |

Commits: 6,678, 2,140.7 µs each: writing the open extents' 319.9 KiB 214.9 µs, committing the store 164.3 µs, sealing the windows' mappings 111.1 µs, waiting for object storage 1,650.1 µs.

Segments closed and framed: 67, 51.6 ms each.

Over 2× slower than ext4:
- sqlite-wal-normal-batch1-checkpoint on fxfs-guest-dax: 2.10× ext4-dax-pmem's
