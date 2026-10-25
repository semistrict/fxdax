Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 185 (184–186) | 825 (807–835) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,758 (2,439–2,809) | 2,644 (2,581–2,705) | 1.04× |
| sqlite-mmap-scan | MiB/s | 3,466 (3,383–3,567) | 3,334 (3,303–3,380) | 1.04× |
| sqlite-mmap-scan-warm | MiB/s | 3,722 (3,602–3,782) | 3,694 (3,582–3,695) | 1.01× |
| sqlite-delete-full-batch1 | txn/s | 161 (155–164) | 267 (260–268) | 0.60× |
| sqlite-delete-full-batch100 | txn/s | 59.1 (57.3–59.2) | 47.1 (46.9–48.0) | 1.26× |
| sqlite-wal-full-batch1 | txn/s | 360 (335–366) | 581 (528–615) | 0.62× |
| sqlite-wal-full-batch1-checkpoint | ms | 14.3 (14.2–15.5) | 20.0 (18.0–47.9) | 1.40× |
| sqlite-wal-full-batch100 | txn/s | 117 (115–120) | 105 (104–106) | 1.11× |
| sqlite-wal-full-batch100-checkpoint | ms | 61.8 (60.8–62.3) | 129 (120–129) | **2.09×** |
| sqlite-wal-normal-batch1 | txn/s | 15,083 (14,791–15,379) | 13,014 (6,649–13,786) | 1.16× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.0 (16.4–17.6) | 31.9 (20.9–74.7) | 1.87× |
| sqlite-wal-normal-batch100 | txn/s | 272 (272–273) | 202 (199–222) | 1.35× |
| sqlite-wal-normal-batch100-checkpoint | ms | 389 (382–392) | 260 (122–318) | 0.67× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -1.61 median (-3.24–-1.24)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 161 median (155–164)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,910 median (5,726–5,923)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 360 median (335–366)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,716 median (11,521–11,993)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 15,083 median (14,791–15,379)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,236 median (27,160–27,304)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.60 median (-4.86–-3.09)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 267 median (260–268)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,709 median (4,692–4,797)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.70 median (5.60–9.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (5.60–9.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 581 median (528–615)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 10,526 median (10,368–10,601)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 13,014 median (6,649–13,786)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 20,226 median (19,943–22,216)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 89 | B | $0.000036 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 127 | A | $0.000635 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 225 | | $0.000706 |

Uploaded 2.0 GiB (segments 2.0 GiB, extents 73.3 MiB, checkpoints 7.0 KiB, moves 0 B, other 39 B); downloaded 2.0 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,671.3 µs |
| read | 4,299 | 73.3 µs |
| write | 33,866 | 370.7 µs |
| free | 617 | 200.9 µs |
| map | 115,770 | 126.0 µs |
| unmap | 22,014 | 26.4 µs |
| commit | 61 | 15,248.2 µs |
| root | 1 | 1.0 µs |
| info | 4 | 1.2 µs |
| window | 1 | 21.3 µs |
| transfer | 1 | 8.0 µs |
| free_many | 1,477 | 263.0 µs |

Commits: 6,732, 1,009.1 µs each: writing the open extents' 314.4 KiB 222.8 µs, committing the store 128.6 µs, sealing the windows' mappings 106.9 µs, waiting for object storage 550.5 µs.

Segments closed and framed: 84, 50.3 ms each.

Over 2× slower than ext4:
- sqlite-wal-full-batch100-checkpoint on fxfs-guest-dax: 2.09× ext4-dax-pmem's
