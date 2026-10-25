Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 184 (184–185) | 832 (743–842) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,771 (2,764–2,774) | 2,705 (2,651–2,710) | 1.02× |
| sqlite-mmap-scan | MiB/s | 3,493 (3,308–3,610) | 3,502 (3,357–3,554) | 1.00× |
| sqlite-mmap-scan-warm | MiB/s | 3,837 (3,807–3,909) | 3,865 (3,832–3,902) | 0.99× |
| sqlite-delete-full-batch1 | txn/s | 169 (168–170) | 266 (266–272) | 0.63× |
| sqlite-delete-full-batch100 | txn/s | 59.3 (59.0–59.9) | 55.0 (54.8–55.1) | 1.08× |
| sqlite-wal-full-batch1 | txn/s | 382 (360–397) | 615 (609–633) | 0.62× |
| sqlite-wal-full-batch1-checkpoint | ms | 13.8 (13.7–14.1) | 15.2 (14.3–18.0) | 1.11× |
| sqlite-wal-full-batch100 | txn/s | 120 (120–121) | 153 (152–154) | 0.78× |
| sqlite-wal-full-batch100-checkpoint | ms | 62.7 (61.8–64.6) | 61.4 (58.7–66.8) | 0.98× |
| sqlite-wal-normal-batch1 | txn/s | 14,942 (14,826–15,217) | 22,417 (20,130–24,287) | 0.67× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.3 (17.2–17.6) | 16.0 (15.5–42.6) | 0.93× |
| sqlite-wal-normal-batch100 | txn/s | 271 (271–271) | 452 (344–453) | 0.60× |
| sqlite-wal-normal-batch100-checkpoint | ms | 373 (371–380) | 188 (95.4–193) | 0.51× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -1.45 median (-4.45–-1.14)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 169 median (168–170)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,933 median (5,899–5,993)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 382 median (360–397)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,998 median (11,979–12,120)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 14,942 median (14,826–15,217)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,128 median (27,111–27,139)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.50 median (-4.99–-3.38)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 266 median (266–272)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,497 median (5,477–5,508)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 615 median (610–633)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 15,297 median (15,158–15,438)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,417 median (20,130–24,287)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 45,195 median (34,421–45,265)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 71 | B | $0.000028 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 316 | A | $0.001580 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 396 | | $0.001644 |

Uploaded 2.0 GiB (segments 1.5 GiB, extents 479.5 MiB, checkpoints 8.1 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,811.7 µs |
| read | 3,977 | 21.9 µs |
| write | 13,447 | 1,023.6 µs |
| free | 600 | 169.8 µs |
| map | 112,420 | 122.6 µs |
| unmap | 17,999 | 29.2 µs |
| commit | 22 | 1,994.6 µs |
| root | 1 | 0.8 µs |
| info | 4 | 1.2 µs |
| window | 1 | 14.3 µs |
| transfer | 1 | 10.9 µs |
| free_many | 1,347 | 281.0 µs |

Commits: 6,685, 1,797.0 µs each: writing the open extents' 319.6 KiB 212.7 µs, committing the store 135.3 µs, sealing the windows' mappings 106.2 µs, waiting for object storage 1,342.4 µs.

Segments closed and framed: 66, 45.7 ms each.

Every workload is within 2× of ext4.
