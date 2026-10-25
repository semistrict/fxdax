Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 184 (181–186) | 849 (791–853) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,743 (2,683–2,769) | 2,796 (2,665–2,828) | 0.98× |
| sqlite-mmap-scan | MiB/s | 3,567 (3,273–3,570) | 3,481 (3,408–3,563) | 1.02× |
| sqlite-mmap-scan-warm | MiB/s | 3,759 (3,744–3,880) | 3,790 (3,780–3,864) | 0.99× |
| sqlite-delete-full-batch1 | txn/s | 176 (159–179) | 265 (264–266) | 0.66× |
| sqlite-delete-full-batch100 | txn/s | 58.7 (56.6–59.8) | 54.0 (53.9–55.8) | 1.09× |
| sqlite-wal-full-batch1 | txn/s | 379 (379–390) | 606 (601–625) | 0.63× |
| sqlite-wal-full-batch1-checkpoint | ms | 13.8 (13.5–14.3) | 15.0 (14.9–15.1) | 1.09× |
| sqlite-wal-full-batch100 | txn/s | 120 (119–125) | 151 (150–158) | 0.80× |
| sqlite-wal-full-batch100-checkpoint | ms | 63.6 (62.8–67.1) | 79.5 (59.7–91.0) | 1.25× |
| sqlite-wal-normal-batch1 | txn/s | 15,010 (14,907–15,063) | 21,750 (19,947–22,790) | 0.69× |
| sqlite-wal-normal-batch1-checkpoint | ms | 16.9 (16.5–17.6) | 18.3 (16.4–38.3) | 1.08× |
| sqlite-wal-normal-batch100 | txn/s | 269 (267–269) | 421 (343–441) | 0.64× |
| sqlite-wal-normal-batch100-checkpoint | ms | 371 (369–466) | 257 (105–265) | 0.69× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.95 median (-6.77–-4.73)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 176 median (159–179)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,873 median (5,663–5,981)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.60–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 379 median (379–390)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 12,039 median (11,932–12,492)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 15,010 median (14,907–15,063)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 26,864 median (26,745–26,906)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.11 median (-6.34–-1.86)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 265 median (264–266)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,404 median (5,395–5,583)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.70 median (3.60–3.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 606 median (601–625)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 15,060 median (14,978–15,766)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 21,750 median (19,947–22,790)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 42,084 median (34,301–44,133)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 73 | B | $0.000029 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 318 | A | $0.001590 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 400 | | $0.001655 |

Uploaded 2.1 GiB (segments 1.6 GiB, extents 479.6 MiB, checkpoints 8.2 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,440.3 µs |
| read | 3,976 | 17.9 µs |
| write | 13,448 | 1,183.8 µs |
| free | 606 | 168.1 µs |
| map | 112,239 | 125.4 µs |
| unmap | 17,980 | 31.4 µs |
| commit | 22 | 2,030.2 µs |
| root | 1 | 1.2 µs |
| info | 4 | 1.3 µs |
| window | 1 | 32.4 µs |
| transfer | 1 | 8.4 µs |
| free_many | 1,361 | 271.1 µs |

Commits: 6,693, 2,174.5 µs each: writing the open extents' 319.2 KiB 214.1 µs, committing the store 158.3 µs, sealing the windows' mappings 109.1 µs, waiting for object storage 1,692.7 µs.

Segments closed and framed: 68, 51.2 ms each.

Every workload is within 2× of ext4.
