Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 183 (181–184) | 839 (720–845) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,811 (2,384–2,819) | 2,579 (2,551–2,638) | 1.09× |
| sqlite-mmap-scan | MiB/s | 3,470 (3,285–3,473) | 3,374 (3,309–3,388) | 1.03× |
| sqlite-mmap-scan-warm | MiB/s | 3,811 (3,809–3,817) | 3,730 (3,684–3,738) | 1.02× |
| sqlite-delete-full-batch1 | txn/s | 180 (174–186) | 258 (253–263) | 0.70× |
| sqlite-delete-full-batch100 | txn/s | 59.3 (57.3–60.4) | 55.1 (53.4–55.2) | 1.08× |
| sqlite-wal-full-batch1 | txn/s | 378 (359–393) | 611 (580–627) | 0.62× |
| sqlite-wal-full-batch1-checkpoint | ms | 13.9 (13.7–14.0) | 19.5 (16.2–42.0) | 1.40× |
| sqlite-wal-full-batch100 | txn/s | 119 (117–120) | 138 (134–144) | 0.86× |
| sqlite-wal-full-batch100-checkpoint | ms | 62.3 (62.0–72.2) | 68.6 (66.0–73.8) | 1.10× |
| sqlite-wal-normal-batch1 | txn/s | 15,199 (15,038–15,336) | 22,880 (22,739–23,862) | 0.66× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.6 (17.0–18.5) | 16.1 (16.0–16.4) | 0.91× |
| sqlite-wal-normal-batch100 | txn/s | 270 (270–270) | 443 (342–445) | 0.61× |
| sqlite-wal-normal-batch100-checkpoint | ms | 380 (373–423) | 207 (90.3–242) | 0.55× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -3.59 median (-6.18–-2.97)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 180 median (174–186)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,930 median (5,727–6,042)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 378 median (359–394)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,926 median (11,672–12,049)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 15,199 median (15,038–15,336)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,012 median (26,979–27,042)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.40 median (-3.47–-3.28)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 258 median (253–263)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,506 median (5,340–5,519)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 611 median (580–626)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 13,793 median (13,406–14,354)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,880 median (22,739–23,862)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 44,269 median (34,177–44,535)

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

Uploaded 2.0 GiB (segments 1.6 GiB, extents 479.7 MiB, checkpoints 10.3 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,002.8 µs |
| read | 3,975 | 45.4 µs |
| write | 13,444 | 1,121.1 µs |
| free | 602 | 187.2 µs |
| map | 111,473 | 149.2 µs |
| unmap | 17,975 | 38.2 µs |
| commit | 22 | 2,096.8 µs |
| root | 1 | 1.8 µs |
| info | 4 | 1.4 µs |
| window | 1 | 14.1 µs |
| transfer | 1 | 15.2 µs |
| free_many | 1,352 | 278.2 µs |

Commits: 6,685, 2,034.9 µs each: writing the open extents' 319.6 KiB 234.0 µs, committing the store 149.9 µs, sealing the windows' mappings 126.7 µs, waiting for object storage 1,524.0 µs.

Segments closed and framed: 67, 48.5 ms each.

Every workload is within 2× of ext4.
