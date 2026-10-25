Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 822 (822–822) |
| sqlite-pread-scan-warm | MiB/s | 2,719 (2,719–2,719) |
| sqlite-mmap-scan | MiB/s | 2,898 (2,898–2,898) |
| sqlite-mmap-scan-warm | MiB/s | 3,149 (3,149–3,149) |
| sqlite-delete-full-batch1 | txn/s | 367 (367–367) |
| sqlite-delete-full-batch100 | txn/s | 60.1 (60.1–60.1) |
| sqlite-wal-full-batch1 | txn/s | 812 (812–812) |
| sqlite-wal-full-batch1-checkpoint | ms | 15.7 (15.7–15.7) |
| sqlite-wal-full-batch100 | txn/s | 139 (139–139) |
| sqlite-wal-full-batch100-checkpoint | ms | 82.7 (82.7–82.7) |
| sqlite-wal-normal-batch1 | txn/s | 17,816 (17,816–17,816) |
| sqlite-wal-normal-batch1-checkpoint | ms | 24.8 (24.8–24.8) |
| sqlite-wal-normal-batch100 | txn/s | 298 (298–298) |
| sqlite-wal-normal-batch100-checkpoint | ms | 261 (261–261) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.61 median (-4.61–-4.61)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 367 median (367–367)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 6,012 median (6,012–6,012)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.70–5.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–5.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 812 median (812–812)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 13,863 median (13,863–13,863)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 17,816 median (17,816–17,816)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 29,815 median (29,815–29,815)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 36 | B | $0.000014 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 7 | A | $0.000035 |
| create | 52 | A | $0.000260 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 100 | | $0.000320 |

Uploaded 714.9 MiB (segments 681.9 MiB, extents 33.0 MiB, checkpoints 449 B, moves 0 B, other 66 B); downloaded 684 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 20,770.4 µs |
| read | 1,565 | 72.7 µs |
| write | 10,839 | 245.2 µs |
| free | 206 | 175.4 µs |
| map | 39,615 | 103.0 µs |
| unmap | 7,397 | 22.3 µs |
| commit | 26 | 11,465.0 µs |
| root | 9 | 0.8 µs |
| info | 11 | 0.8 µs |
| window | 2 | 13.2 µs |
| transfer | 2 | 9.5 µs |
| free_many | 488 | 210.4 µs |

Commits: 2,256, 923.0 µs each: writing the open extents' 312.7 KiB 230.3 µs, committing the store 100.9 µs, sealing the windows' mappings 101.3 µs, waiting for object storage 490.1 µs.

Segments closed and framed: 27, 54.1 ms each.

