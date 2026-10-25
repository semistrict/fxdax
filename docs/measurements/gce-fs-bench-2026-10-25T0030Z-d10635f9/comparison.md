Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 5.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 828 (801–845) |
| sqlite-pread-scan-warm | MiB/s | 2,576 (2,552–2,673) |
| sqlite-mmap-scan | MiB/s | 3,389 (3,322–3,422) |
| sqlite-mmap-scan-warm | MiB/s | 3,707 (3,600–3,769) |
| sqlite-delete-full-batch1 | txn/s | 249 (232–252) |
| sqlite-delete-full-batch100 | txn/s | 42.0 (40.0–43.7) |
| sqlite-wal-full-batch1 | txn/s | 574 (480–603) |
| sqlite-wal-full-batch1-checkpoint | ms | 34.8 (18.6–41.9) |
| sqlite-wal-full-batch100 | txn/s | 93.8 (91.9–95.5) |
| sqlite-wal-full-batch100-checkpoint | ms | 110 (93.0–127) |
| sqlite-wal-normal-batch1 | txn/s | 11,974 (10,572–12,464) |
| sqlite-wal-normal-batch1-checkpoint | ms | 30.2 (21.6–38.1) |
| sqlite-wal-normal-batch100 | txn/s | 168 (165–174) |
| sqlite-wal-normal-batch100-checkpoint | ms | 206 (133–319) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.15 median (-6.08–-3.91)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 249 median (232–252)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,202 median (4,003–4,370)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (3.70–9.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (3.60–9.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 574 median (480–604)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 9,384 median (9,192–9,545)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 11,974 median (10,572–12,464)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 16,817 median (16,529–17,413)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 159 | B | $0.000064 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 7 | A | $0.000035 |
| create | 459 | A | $0.002295 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 629 | | $0.002404 |

Uploaded 3.3 GiB (segments 3.2 GiB, extents 116.2 MiB, checkpoints 22.3 KiB, moves 0 B, other 39 B); downloaded 3.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 9,270.5 µs |
| read | 7,168 | 49.9 µs |
| write | 56,709 | 362.3 µs |
| free | 1,033 | 201.5 µs |
| map | 193,801 | 131.2 µs |
| unmap | 36,544 | 26.7 µs |
| commit | 100 | 14,535.8 µs |
| root | 1 | 0.7 µs |
| info | 4 | 0.8 µs |
| window | 1 | 26.2 µs |
| transfer | 1 | 9.2 µs |
| free_many | 2,460 | 260.4 µs |

Commits: 11,224, 855.8 µs each: writing the open extents' 314.3 KiB 216.8 µs, committing the store 143.2 µs, sealing the windows' mappings 111.6 µs, waiting for object storage 383.9 µs.

Segments closed and framed: 155, 43.0 ms each.

