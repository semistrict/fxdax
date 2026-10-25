Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 5.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 833 (803–841) |
| sqlite-pread-scan-warm | MiB/s | 2,660 (2,623–2,696) |
| sqlite-mmap-scan | MiB/s | 3,333 (3,297–3,421) |
| sqlite-mmap-scan-warm | MiB/s | 3,672 (3,633–3,732) |
| sqlite-delete-full-batch1 | txn/s | 253 (243–255) |
| sqlite-delete-full-batch100 | txn/s | 43.4 (40.0–44.1) |
| sqlite-wal-full-batch1 | txn/s | 608 (564–642) |
| sqlite-wal-full-batch1-checkpoint | ms | 23.2 (18.3–29.0) |
| sqlite-wal-full-batch100 | txn/s | 93.4 (91.0–94.5) |
| sqlite-wal-full-batch100-checkpoint | ms | 108 (95.0–120) |
| sqlite-wal-normal-batch1 | txn/s | 12,051 (7,731–13,137) |
| sqlite-wal-normal-batch1-checkpoint | ms | 30.1 (16.7–37.2) |
| sqlite-wal-normal-batch100 | txn/s | 168 (161–174) |
| sqlite-wal-normal-batch100-checkpoint | ms | 185 (100–325) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.53 median (-6.78–-4.20)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 253 median (243–255)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,336 median (4,004–4,406)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.60–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 608 median (564–642)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 9,338 median (9,102–9,453)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 12,051 median (7,731–13,138)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 16,818 median (16,114–17,444)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 154 | B | $0.000062 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 7 | A | $0.000035 |
| create | 379 | A | $0.001895 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 544 | | $0.002002 |

Uploaded 3.3 GiB (segments 3.2 GiB, extents 117.0 MiB, checkpoints 15.8 KiB, moves 0 B, other 39 B); downloaded 3.5 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,870.6 µs |
| read | 7,177 | 48.1 µs |
| write | 56,712 | 361.8 µs |
| free | 1,025 | 185.1 µs |
| map | 192,873 | 128.5 µs |
| unmap | 36,570 | 25.3 µs |
| commit | 100 | 15,018.3 µs |
| root | 1 | 0.7 µs |
| info | 4 | 0.9 µs |
| window | 1 | 25.6 µs |
| transfer | 1 | 9.0 µs |
| free_many | 2,451 | 245.5 µs |

Commits: 11,219, 876.0 µs each: writing the open extents' 314.4 KiB 220.2 µs, committing the store 151.4 µs, sealing the windows' mappings 109.7 µs, waiting for object storage 394.4 µs.

Segments closed and framed: 151, 45.1 ms each.

