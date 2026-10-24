Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 783 (757–785) |
| sqlite-pread-scan-warm | MiB/s | 2,623 (2,609–2,711) |
| sqlite-mmap-scan | MiB/s | 3,296 (3,236–3,475) |
| sqlite-mmap-scan-warm | MiB/s | 3,672 (3,593–3,686) |
| sqlite-delete-full-batch1 | txn/s | 253 (246–255) |
| sqlite-delete-full-batch100 | txn/s | 41.6 (40.1–41.8) |
| sqlite-wal-full-batch1 | txn/s | 515 (476–531) |
| sqlite-wal-full-batch1-checkpoint | ms | 19.9 (19.8–20.2) |
| sqlite-wal-full-batch100 | txn/s | 90.9 (87.9–92.8) |
| sqlite-wal-full-batch100-checkpoint | ms | 107 (102–108) |
| sqlite-wal-normal-batch1 | txn/s | 10,803 (8,703–11,299) |
| sqlite-wal-normal-batch1-checkpoint | ms | 36.6 (33.0–50.4) |
| sqlite-wal-normal-batch100 | txn/s | 168 (168–169) |
| sqlite-wal-normal-batch100-checkpoint | ms | 249 (129–301) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.53 median (-6.36–-5.44)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 253 median (246–255)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,164 median (4,010–4,176)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.70–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 516 median (476–531)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 9,090 median (8,787–9,280)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 10,803 median (8,703–11,299)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 16,814 median (16,800–16,947)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 100 | B | $0.000040 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 141 | A | $0.000705 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 250 | | $0.000780 |

Uploaded 2.0 GiB (segments 1.9 GiB, extents 76.1 MiB, checkpoints 9.9 KiB, moves 0 B, other 39 B); downloaded 2.3 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 9,358.0 µs |
| read | 4,292 | 62.1 µs |
| write | 33,988 | 424.7 µs |
| free | 612 | 199.7 µs |
| map | 125,286 | 119.7 µs |
| unmap | 21,865 | 29.5 µs |
| commit | 62 | 15,164.2 µs |
| root | 1 | 0.9 µs |
| info | 4 | 1.2 µs |
| window | 1 | 14.7 µs |
| fault | 3 | 24.2 µs |
| transfer | 1 | 11.5 µs |
| free_many | 1,473 | 251.8 µs |

Commits: 6,732, 898.0 µs each: writing the open extents' 314.4 KiB 225.6 µs, committing the store 153.7 µs, sealing the windows' mappings 117.2 µs, waiting for object storage 401.2 µs.

Segments closed and framed: 95, 44.4 ms each.

