Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 5.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 824 (792–842) |
| sqlite-pread-scan-warm | MiB/s | 2,680 (2,594–2,688) |
| sqlite-mmap-scan | MiB/s | 3,337 (3,288–3,355) |
| sqlite-mmap-scan-warm | MiB/s | 3,676 (3,625–3,758) |
| sqlite-delete-full-batch1 | txn/s | 254 (250–261) |
| sqlite-delete-full-batch100 | txn/s | 46.2 (45.2–47.0) |
| sqlite-wal-full-batch1 | txn/s | 591 (582–599) |
| sqlite-wal-full-batch1-checkpoint | ms | 19.9 (17.4–22.7) |
| sqlite-wal-full-batch100 | txn/s | 102 (98.6–104) |
| sqlite-wal-full-batch100-checkpoint | ms | 118 (93.5–140) |
| sqlite-wal-normal-batch1 | txn/s | 12,492 (6,942–13,285) |
| sqlite-wal-normal-batch1-checkpoint | ms | 31.3 (17.2–35.4) |
| sqlite-wal-normal-batch100 | txn/s | 188 (181–217) |
| sqlite-wal-normal-batch100-checkpoint | ms | 185 (128–290) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.39 median (-5.85–-3.73)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 254 median (250–261)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,622 median (4,515–4,705)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.60–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 591 median (582–599)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 10,212 median (9,860–10,370)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 12,492 median (6,942–13,285)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 18,798 median (18,084–21,680)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 149 | B | $0.000060 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 6 | A | $0.000030 |
| create | 212 | A | $0.001060 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 371 | | $0.001160 |

Uploaded 3.2 GiB (segments 3.1 GiB, extents 121.0 MiB, checkpoints 11.3 KiB, moves 0 B, other 39 B); downloaded 3.4 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 9,256.1 µs |
| read | 7,169 | 35.1 µs |
| write | 56,527 | 343.6 µs |
| free | 1,026 | 189.7 µs |
| map | 192,484 | 128.6 µs |
| unmap | 36,682 | 26.2 µs |
| commit | 99 | 15,325.8 µs |
| root | 1 | 1.1 µs |
| info | 4 | 1.1 µs |
| window | 1 | 20.4 µs |
| transfer | 1 | 9.6 µs |
| free_many | 2,448 | 266.2 µs |

Commits: 11,218, 814.2 µs each: writing the open extents' 314.4 KiB 221.1 µs, committing the store 111.2 µs, sealing the windows' mappings 110.2 µs, waiting for object storage 371.5 µs.

Segments closed and framed: 145, 45.1 ms each.

