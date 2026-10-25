Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 5, fxfs-guest-dax 5.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 185 (182–185) | 846 (755–851) | 0.22× |
| sqlite-pread-scan-warm | MiB/s | 2,719 (2,682–2,813) | 2,806 (2,779–2,818) | 0.97× |
| sqlite-mmap-scan | MiB/s | 3,466 (3,421–3,516) | 3,437 (3,354–3,499) | 1.01× |
| sqlite-mmap-scan-warm | MiB/s | 3,789 (3,764–3,825) | 3,826 (3,790–3,883) | 0.99× |
| sqlite-delete-full-batch1 | txn/s | 179 (168–184) | 267 (260–274) | 0.67× |
| sqlite-delete-full-batch100 | txn/s | 58.7 (57.5–59.2) | 54.3 (53.6–56.2) | 1.08× |
| sqlite-wal-full-batch1 | txn/s | 376 (369–391) | 609 (590–636) | 0.62× |
| sqlite-wal-full-batch1-checkpoint | ms | 14.0 (13.8–14.3) | 17.4 (14.5–19.8) | 1.24× |
| sqlite-wal-full-batch100 | txn/s | 120 (118–121) | 159 (142–163) | 0.75× |
| sqlite-wal-full-batch100-checkpoint | ms | 62.5 (61.9–63.8) | 64.0 (60.5–65.6) | 1.02× |
| sqlite-wal-normal-batch1 | txn/s | 14,892 (14,609–15,600) | 22,239 (20,523–23,763) | 0.67× |
| sqlite-wal-normal-batch1-checkpoint | ms | 17.6 (16.8–19.2) | 19.6 (15.7–37.4) | 1.12× |
| sqlite-wal-normal-batch100 | txn/s | 270 (270–273) | 449 (352–461) | 0.60× |
| sqlite-wal-normal-batch100-checkpoint | ms | 380 (369–384) | 195 (109–257) | 0.51× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.21 median (-5.70–-3.18)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 179 median (168–184)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,868 median (5,746–5,917)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.70)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 376 median (369–391)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,969 median (11,758–12,118)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 14,892 median (14,609–15,600)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 27,033 median (26,994–27,266)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.93 median (-5.86–-2.57)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 267 median (260–274)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 5,431 median (5,362–5,620)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 609 median (590–636)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 15,876 median (14,156–16,300)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 22,239 median (20,523–23,763)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 44,942 median (35,244–46,091)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 116 | B | $0.000046 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 4 | A | $0.000020 |
| create | 522 | A | $0.002610 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 646 | | $0.002687 |

Uploaded 3.3 GiB (segments 2.6 GiB, extents 799.1 MiB, checkpoints 4.3 KiB, moves 0 B, other 39 B); downloaded 2.6 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,171.2 µs |
| read | 6,628 | 13.5 µs |
| write | 22,409 | 1,086.7 µs |
| free | 995 | 172.0 µs |
| map | 186,020 | 125.3 µs |
| unmap | 29,949 | 29.9 µs |
| commit | 34 | 1,202.0 µs |
| root | 1 | 0.7 µs |
| info | 4 | 1.3 µs |
| window | 1 | 12.6 µs |
| transfer | 1 | 12.8 µs |
| free_many | 2,261 | 272.4 µs |

Commits: 11,132, 1,946.8 µs each: writing the open extents' 319.9 KiB 215.1 µs, committing the store 140.0 µs, sealing the windows' mappings 108.1 µs, waiting for object storage 1,483.3 µs.

Segments closed and framed: 111, 46.4 ms each.

Every workload is within 2× of ext4.
