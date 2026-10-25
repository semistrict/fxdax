Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 183 (181–185) | 818 (782–851) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,777 (2,693–2,815) | 2,706 (2,482–2,718) | 1.03× |
| sqlite-mmap-scan | MiB/s | 3,437 (3,346–3,495) | 3,497 (3,314–3,525) | 0.98× |
| sqlite-mmap-scan-warm | MiB/s | 3,818 (3,766–3,845) | 3,774 (3,667–3,872) | 1.01× |
| sqlite-delete-full-batch1 | txn/s | 169 (159–176) | 260 (259–263) | 0.65× |
| sqlite-delete-full-batch100 | txn/s | 59.3 (58.7–64.5) | 55.1 (54.9–55.3) | 1.08× |
| sqlite-wal-full-batch1 | txn/s | 434 (378–469) | 619 (607–634) | 0.70× |
| sqlite-wal-full-batch1-checkpoint | ms | 13.7 (13.3–13.8) | 20.4 (19.4–23.2) | 1.49× |
| sqlite-wal-full-batch100 | txn/s | 117 (116–127) | 142 (140–143) | 0.82× |
| sqlite-wal-full-batch100-checkpoint | ms | 63.3 (62.9–64.0) | 86.9 (72.2–91.9) | 1.37× |
| sqlite-wal-normal-batch1 | txn/s | 14,879 (14,630–15,509) | 22,675 (19,720–24,547) | 0.66× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.3 (17.1–18.2) | 14.8 (14.5–32.1) | 0.85× |
| sqlite-wal-normal-batch100 | txn/s | 269 (267–270) | 448 (433–460) | 0.60× |
| sqlite-wal-normal-batch100-checkpoint | ms | 371 (369–379) | 265 (214–279) | 0.71× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -1.49 median (-2.13–-0.95)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 169 median (159–176)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,929 median (5,865–6,452)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.60–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.70)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 434 median (378–469)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,708 median (11,588–12,690)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 14,879 median (14,630–15,509)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 26,859 median (26,730–27,048)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.11 median (-5.12–-3.57)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 260 median (259–263)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,513 median (5,488–5,530)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–5.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.70 median (3.60–5.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 619 median (606–634)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 14,218 median (14,023–14,311)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,675 median (19,720–24,547)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 44,824 median (43,332–46,037)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 72 | B | $0.000029 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 302 | A | $0.001510 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 383 | | $0.001574 |

Uploaded 2.1 GiB (segments 1.7 GiB, extents 450.0 MiB, checkpoints 12.5 KiB, moves 0 B, other 39 B); downloaded 1.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,504.8 µs |
| read | 7,401 | 10.4 µs |
| write | 14,460 | 1,109.6 µs |
| free | 608 | 175.9 µs |
| map | 112,362 | 125.4 µs |
| unmap | 18,048 | 31.6 µs |
| commit | 20 | 1,154.4 µs |
| root | 1 | 0.6 µs |
| info | 4 | 1.3 µs |
| window | 1 | 13.7 µs |
| transfer | 1 | 10.0 µs |
| free_many | 1,337 | 280.9 µs |

Commits: 6,677, 2,139.4 µs each: writing the open extents' 320.2 KiB 223.3 µs, committing the store 159.2 µs, sealing the windows' mappings 112.8 µs, waiting for object storage 1,643.8 µs.

Segments closed and framed: 67, 56.5 ms each.

Every workload is within 2× of ext4.
