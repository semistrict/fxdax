Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 5.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 827 (801–842) |
| sqlite-pread-scan-warm | MiB/s | 2,644 (2,524–2,693) |
| sqlite-mmap-scan | MiB/s | 3,374 (3,230–3,438) |
| sqlite-mmap-scan-warm | MiB/s | 3,717 (3,669–3,758) |
| sqlite-delete-full-batch1 | txn/s | 258 (249–265) |
| sqlite-delete-full-batch100 | txn/s | 46.7 (45.8–47.2) |
| sqlite-wal-full-batch1 | txn/s | 565 (499–606) |
| sqlite-wal-full-batch1-checkpoint | ms | 19.1 (16.9–20.5) |
| sqlite-wal-full-batch100 | txn/s | 104 (103–104) |
| sqlite-wal-full-batch100-checkpoint | ms | 97.4 (91.6–162) |
| sqlite-wal-normal-batch1 | txn/s | 11,878 (8,103–13,572) |
| sqlite-wal-normal-batch1-checkpoint | ms | 34.4 (16.2–38.0) |
| sqlite-wal-normal-batch100 | txn/s | 202 (162–220) |
| sqlite-wal-normal-batch100-checkpoint | ms | 164 (121–277) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.56 median (-5.42–-2.05)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 258 median (249–265)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,670 median (4,582–4,719)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.60 median (3.70–5.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (3.60–5.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 565 median (499–606)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 10,377 median (10,285–10,394)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 11,878 median (8,103–13,572)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 20,173 median (16,195–21,994)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 150 | B | $0.000060 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 6 | A | $0.000030 |
| create | 212 | A | $0.001060 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 372 | | $0.001160 |

Uploaded 3.3 GiB (segments 3.1 GiB, extents 118.6 MiB, checkpoints 12.1 KiB, moves 0 B, other 39 B); downloaded 3.4 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,404.3 µs |
| read | 7,172 | 42.3 µs |
| write | 56,531 | 358.2 µs |
| free | 1,028 | 183.6 µs |
| map | 190,693 | 127.2 µs |
| unmap | 36,299 | 24.8 µs |
| commit | 99 | 15,027.8 µs |
| root | 1 | 0.8 µs |
| info | 4 | 1.1 µs |
| window | 1 | 20.5 µs |
| transfer | 1 | 9.2 µs |
| free_many | 2,458 | 256.6 µs |

Commits: 11,227, 853.0 µs each: writing the open extents' 314.2 KiB 218.8 µs, committing the store 149.8 µs, sealing the windows' mappings 106.7 µs, waiting for object storage 377.4 µs.

Segments closed and framed: 147, 42.7 ms each.

