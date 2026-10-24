Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 153 (134–156) |
| rand-read-cold | IOPS | 92,565 (76,920–105,703) |
| seq-read-cold | MiB/s | 610 (493–699) |
| seq-read-warm | MiB/s | 8,526 (8,313–8,582) |
| rand-read-warm | IOPS | 812,930 (805,433–814,320) |
| mmap-seq-read | MiB/s | 8,258 (7,529–8,258) |
| rand-write | IOPS | 18,431 (14,307–18,435) |
| mmap-rand-read | IOPS | 515,684 (333,054–728,671) |
| mmap-rand-write | IOPS | 13,844 (11,808–13,960) |
| fsync-append | IOPS | 390 (385–392) |
| sqlite-pread-scan | MiB/s | 817 (791–825) |
| sqlite-pread-scan-warm | MiB/s | 2,640 (2,634–2,649) |
| sqlite-mmap-scan | MiB/s | 3,352 (3,312–3,430) |
| sqlite-mmap-scan-warm | MiB/s | 3,683 (3,578–3,731) |

- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.25 median (-4.44–-3.32)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.60 median (3.70–7.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (3.70–7.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 54 | B | $0.000022 |
| get-range | 1 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 582 | A | $0.002910 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 643 | | $0.002957 |

Uploaded 1.5 GiB (segments 1.0 GiB, extents 516.5 MiB, checkpoints 0 B, moves 0 B, other 39 B); downloaded 1.2 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 8,521.4 µs |
| read | 194 | 142.5 µs |
| write | 13,360 | 889.2 µs |
| free | 12 | 98.3 µs |
| map | 110,317 | 164.3 µs |
| unmap | 5,693 | 65.7 µs |
| commit | 28 | 32,643.2 µs |
| root | 1 | 1.0 µs |
| info | 4 | 1.0 µs |
| window | 1 | 14.6 µs |
| transfer | 1 | 11.9 µs |
| free_many | 76 | 7,051.0 µs |

Commits: 842, 13,653.8 µs each: writing the open extents' 2,341.2 KiB 1,287.3 µs, committing the store 3,357.9 µs, sealing the windows' mappings 336.5 µs, waiting for object storage 8,671.8 µs.

Segments closed and framed: 49, 52.0 ms each.

