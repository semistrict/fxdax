Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 185 (183–185) | 832 (738–843) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,757 (2,642–2,773) | 2,541 (2,438–2,695) | 1.08× |
| sqlite-mmap-scan | MiB/s | 3,534 (3,473–3,537) | 3,472 (3,433–3,479) | 1.02× |
| sqlite-mmap-scan-warm | MiB/s | 3,836 (3,799–3,886) | 3,843 (3,746–3,860) | 1.00× |
| sqlite-delete-full-batch1 | txn/s | 171 (165–176) | 264 (262–269) | 0.65× |
| sqlite-delete-full-batch100 | txn/s | 60.3 (58.4–60.6) | 55.4 (54.9–55.4) | 1.09× |
| sqlite-wal-full-batch1 | txn/s | 391 (386–397) | 616 (552–622) | 0.63× |
| sqlite-wal-full-batch1-checkpoint | ms | 14.0 (13.6–14.7) | 20.0 (17.0–21.4) | 1.43× |
| sqlite-wal-full-batch100 | txn/s | 120 (120–122) | 150 (139–151) | 0.80× |
| sqlite-wal-full-batch100-checkpoint | ms | 62.1 (61.9–62.3) | 66.5 (64.4–70.0) | 1.07× |
| sqlite-wal-normal-batch1 | txn/s | 15,086 (14,817–15,332) | 22,337 (20,824–22,659) | 0.68× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.1 (17.0–17.8) | 16.7 (16.1–37.4) | 0.98× |
| sqlite-wal-normal-batch100 | txn/s | 270 (270–270) | 441 (342–444) | 0.61× |
| sqlite-wal-normal-batch100-checkpoint | ms | 372 (369–385) | 186 (88.6–252) | 0.50× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -3.58 median (-3.79–-3.12)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 171 median (165–176)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 6,029 median (5,843–6,059)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.60–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 391 median (386–397)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 12,046 median (11,985–12,213)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 15,086 median (14,817–15,332)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,018 median (27,002–27,048)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.73 median (-5.94–-4.71)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 264 median (262–269)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,541 median (5,494–5,544)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.70 median (3.60–3.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 616 median (552–622)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 14,978 median (13,888–15,066)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,338 median (20,824–22,659)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 44,138 median (34,152–44,387)

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

Uploaded 2.0 GiB (segments 1.6 GiB, extents 479.5 MiB, checkpoints 8.8 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,057.0 µs |
| read | 3,977 | 10.5 µs |
| write | 13,446 | 1,120.7 µs |
| free | 599 | 170.1 µs |
| map | 113,021 | 127.9 µs |
| unmap | 17,985 | 31.0 µs |
| commit | 22 | 1,856.0 µs |
| root | 1 | 1.0 µs |
| info | 4 | 1.2 µs |
| window | 1 | 13.8 µs |
| transfer | 1 | 12.0 µs |
| free_many | 1,347 | 273.4 µs |

Commits: 6,684, 1,945.2 µs each: writing the open extents' 319.6 KiB 238.5 µs, committing the store 150.9 µs, sealing the windows' mappings 111.5 µs, waiting for object storage 1,444.0 µs.

Segments closed and framed: 67, 49.6 ms each.

Every workload is within 2× of ext4.
