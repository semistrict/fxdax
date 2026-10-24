Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| sqlite-pread-scan | MiB/s | 719 (719–719) |
| sqlite-pread-scan-warm | MiB/s | 2,366 (2,366–2,366) |
| sqlite-mmap-scan | MiB/s | 2,666 (2,666–2,666) |
| sqlite-mmap-scan-warm | MiB/s | 2,813 (2,813–2,813) |
| sqlite-delete-full-batch1 | txn/s | 321 (321–321) |
| sqlite-delete-full-batch100 | txn/s | 49.7 (49.7–49.7) |
| sqlite-wal-full-batch1 | txn/s | 728 (728–728) |
| sqlite-wal-full-batch1-checkpoint | ms | 21.2 (21.2–21.2) |
| sqlite-wal-full-batch100 | txn/s | 105 (105–105) |
| sqlite-wal-full-batch100-checkpoint | ms | 106 (106–106) |
| sqlite-wal-normal-batch1 | txn/s | 13,872 (13,872–13,872) |
| sqlite-wal-normal-batch1-checkpoint | ms | 18.4 (18.4–18.4) |
| sqlite-wal-normal-batch100 | txn/s | 177 (177–177) |
| sqlite-wal-normal-batch100-checkpoint | ms | 130 (130–130) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.04 median (-6.04–-6.04)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 321 median (321–321)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,971 median (4,971–4,971)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 17.3 median (17.3–17.3)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 17.2 median (17.2–17.2)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 16.3 median (16.3–16.3)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 16.2 median (16.2–16.2)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 728 median (728–728)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 10,540 median (10,540–10,540)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 13,872 median (13,872–13,872)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 17,661 median (17,661–17,661)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 39 | B | $0.000016 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 4 | A | $0.000020 |
| create | 57 | A | $0.000285 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 104 | | $0.000331 |

Uploaded 678.7 MiB (segments 645.0 MiB, extents 33.7 MiB, checkpoints 4.2 KiB, moves 0 B, other 39 B); downloaded 846 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 7,548.3 µs |
| read | 1,422 | 22.8 µs |
| write | 11,240 | 272.0 µs |
| free | 204 | 191.0 µs |
| map | 41,728 | 105.0 µs |
| unmap | 7,733 | 20.9 µs |
| commit | 22 | 15,736.7 µs |
| root | 1 | 1.1 µs |
| info | 4 | 1.0 µs |
| window | 1 | 13.5 µs |
| transfer | 1 | 17.9 µs |
| free_many | 483 | 256.7 µs |

Commits: 2,246, 871.5 µs each: writing the open extents' 314.1 KiB 214.0 µs, committing the store 91.5 µs, sealing the windows' mappings 114.7 µs, waiting for object storage 450.9 µs.

Segments closed and framed: 34, 39.6 ms each.

