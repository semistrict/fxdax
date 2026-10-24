Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 794 (790–819) |
| sqlite-pread-scan-warm | MiB/s | 2,649 (2,622–2,691) |
| sqlite-mmap-scan | MiB/s | 3,332 (3,214–3,376) |
| sqlite-mmap-scan-warm | MiB/s | 3,612 (3,556–3,695) |
| sqlite-delete-full-batch1 | txn/s | 254 (252–265) |
| sqlite-delete-full-batch100 | txn/s | 43.2 (42.8–43.5) |
| sqlite-wal-full-batch1 | txn/s | 589 (556–616) |
| sqlite-wal-full-batch1-checkpoint | ms | 19.8 (15.2–20.3) |
| sqlite-wal-full-batch100 | txn/s | 94.4 (93.7–95.9) |
| sqlite-wal-full-batch100-checkpoint | ms | 111 (95.0–143) |
| sqlite-wal-normal-batch1 | txn/s | 12,156 (11,982–12,692) |
| sqlite-wal-normal-batch1-checkpoint | ms | 33.2 (16.9–38.4) |
| sqlite-wal-normal-batch100 | txn/s | 170 (170–171) |
| sqlite-wal-normal-batch100-checkpoint | ms | 161 (98.2–221) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.84 median (-5.09–-4.15)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 254 median (252–264)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,321 median (4,282–4,351)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.60–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 589 median (556–616)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 9,436 median (9,371–9,590)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 12,156 median (11,982–12,692)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 17,026 median (16,955–17,150)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 95 | B | $0.000038 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 6 | A | $0.000030 |
| create | 248 | A | $0.001240 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 353 | | $0.001318 |

Uploaded 2.0 GiB (segments 1.9 GiB, extents 76.0 MiB, checkpoints 12.8 KiB, moves 0 B, other 39 B); downloaded 2.1 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,850.9 µs |
| read | 4,306 | 29.3 µs |
| write | 33,992 | 337.8 µs |
| free | 612 | 189.3 µs |
| map | 117,979 | 130.6 µs |
| unmap | 22,069 | 25.3 µs |
| commit | 61 | 15,468.1 µs |
| root | 1 | 0.8 µs |
| info | 4 | 0.7 µs |
| window | 1 | 13.2 µs |
| transfer | 1 | 10.9 µs |
| free_many | 1,478 | 263.9 µs |

Commits: 6,736, 944.1 µs each: writing the open extents' 314.2 KiB 228.6 µs, committing the store 121.6 µs, sealing the windows' mappings 109.0 µs, waiting for object storage 484.6 µs.

Segments closed and framed: 90, 46.4 ms each.

