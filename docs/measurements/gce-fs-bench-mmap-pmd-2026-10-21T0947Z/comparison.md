Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–119) | 281 (276–283) | 0.38× |
| rand-read-cold | IOPS | 75,502 (75,502–76,205) | 147,604 (110,703–156,038) | 0.51× |
| seq-read-cold | MiB/s | 453 (452–457) | 988 (711–1,080) | 0.46× |
| seq-read-warm | MiB/s | 11,612 (11,354–11,662) | 11,110 (10,530–11,639) | 1.05× |
| rand-read-warm | IOPS | 897,439 (896,707–899,670) | 812,530 (746,203–813,593) | 1.10× |
| mmap-seq-read | MiB/s | 10,240 (10,240–10,667) | 9,846 (1,984–10,240) | 1.04× |
| rand-write | IOPS | 8,685 (8,682–8,686) | 18,452 (14,418–18,482) | 0.47× |
| mmap-rand-read | IOPS | 832,310 (822,233–837,647) | 1,121,453 (1,052,167–1,127,464) | 0.74× |
| mmap-rand-write | IOPS | 8,686 (8,685–8,687) | 18,488 (14,415–18,496) | 0.47× |
| fsync-append | IOPS | 346 (346–346) | 403 (402–642) | 0.86× |
| sqlite-mmap-scan | MiB/s | 310 (293–314) | 707 (701–707) | 0.44× |

- ext4-dax-pmem: mmap-seq-read-faults-per-mib 5.60 median (5.60–5.60)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.84 median (-6.70–-3.68)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–129)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–261)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.47 median (-5.81–-4.17)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 256 median (256–256)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 68 | B | $0.000027 |
| get-range | 0 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 67 | A | $0.000335 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 141 | | $0.000387 |

Uploaded 1.4 GiB (segments 1.4 GiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 1.5 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 12,675.5 µs |
| read | 200 | 1,126.9 µs |
| write | 13,553 | 1,349.9 µs |
| free | 13,467 | 22.6 µs |
| map | 10,458 | 62.4 µs |
| unmap | 6,932 | 84.3 µs |
| commit | 28 | 485,710.9 µs |
| root | 1 | 0.9 µs |
| info | 4 | 1.0 µs |
| window | 1 | 38.8 µs |
| transfer | 1 | 19.1 µs |

Commits: 839, 37,821.7 µs each: writing the open extents' 2,349.6 KiB 1,149.7 µs, committing the store 66.9 µs, sealing the windows' mappings 664.4 µs, waiting for object storage 35,940.4 µs.

Segments closed and framed: 63, 290.5 ms each.

Every workload is within 2× of ext4.
