Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 788 (730–841) |
| sqlite-pread-scan-warm | MiB/s | 2,667 (2,527–2,671) |
| sqlite-mmap-scan | MiB/s | 3,270 (3,253–3,332) |
| sqlite-mmap-scan-warm | MiB/s | 3,647 (3,646–3,673) |
| sqlite-delete-full-batch1 | txn/s | 219 (211–222) |
| sqlite-delete-full-batch100 | txn/s | 45.8 (44.9–46.4) |
| sqlite-wal-full-batch1 | txn/s | 533 (519–543) |
| sqlite-wal-full-batch1-checkpoint | ms | 36.1 (20.0–39.3) |
| sqlite-wal-full-batch100 | txn/s | 100 (98.0–102) |
| sqlite-wal-full-batch100-checkpoint | ms | 114 (106–124) |
| sqlite-wal-normal-batch1 | txn/s | 12,260 (12,051–12,531) |
| sqlite-wal-normal-batch1-checkpoint | ms | 30.5 (18.4–37.9) |
| sqlite-wal-normal-batch100 | txn/s | 175 (169–186) |
| sqlite-wal-normal-batch100-checkpoint | ms | 154 (122–211) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.75 median (-7.64–-3.31)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 219 median (211–222)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,580 median (4,489–4,643)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 7.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 7.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 534 median (518–543)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 10,038 median (9,796–10,216)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 12,260 median (12,051–12,532)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 17,460 median (16,939–18,615)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 94 | B | $0.000038 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 6 | A | $0.000030 |
| create | 133 | A | $0.000665 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 237 | | $0.000743 |

Uploaded 2.0 GiB (segments 1.9 GiB, extents 73.4 MiB, checkpoints 11.4 KiB, moves 0 B, other 39 B); downloaded 2.1 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,631.8 µs |
| read | 4,297 | 35.6 µs |
| write | 33,991 | 411.2 µs |
| free | 617 | 201.9 µs |
| map | 109,798 | 147.2 µs |
| unmap | 22,172 | 28.4 µs |
| commit | 62 | 15,560.1 µs |
| root | 1 | 0.7 µs |
| info | 4 | 1.4 µs |
| window | 1 | 12.2 µs |
| transfer | 1 | 7.9 µs |
| free_many | 1,472 | 351.3 µs |

Commits: 6,736, 917.1 µs each: writing the open extents' 314.2 KiB 246.3 µs, committing the store 142.4 µs, sealing the windows' mappings 118.3 µs, waiting for object storage 409.7 µs.

Segments closed and framed: 89, 44.6 ms each.

