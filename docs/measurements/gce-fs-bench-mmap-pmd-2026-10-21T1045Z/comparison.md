Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–107) | 283 (282–283) | 0.38× |
| rand-read-cold | IOPS | 76,561 (75,852–76,920) | 130,032 (112,993–160,627) | 0.59× |
| seq-read-cold | MiB/s | 456 (449–458) | 810 (717–1,049) | 0.56× |
| seq-read-warm | MiB/s | 11,660 (11,509–11,767) | 11,555 (11,242–11,654) | 1.01× |
| rand-read-warm | IOPS | 899,451 (894,603–902,647) | 834,462 (801,055–839,165) | 1.08× |
| mmap-seq-read | MiB/s | 10,240 (10,240–10,667) | 10,240 (10,240–10,240) | 1.00× |
| rand-write | IOPS | 8,686 (7,663–8,687) | 18,483 (14,421–18,494) | 0.47× |
| mmap-rand-read | IOPS | 833,211 (819,182–837,668) | 1,131,700 (1,117,993–1,168,199) | 0.74× |
| mmap-rand-write | IOPS | 8,683 (7,668–8,686) | 18,486 (14,419–18,493) | 0.47× |
| fsync-append | IOPS | 346 (346–346) | 402 (402–662) | 0.86× |
| sqlite-mmap-scan | MiB/s | 312 (291–316) | 703 (695–705) | 0.44× |

- ext4-dax-pmem: mmap-seq-read-faults-per-mib 5.60 median (5.60–5.60)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -5.39 median (-6.16–-4.69)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–129)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -10.51 median (-16.45–-4.27)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 256 median (256–256)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 56 | B | $0.000022 |
| get-range | 0 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 55 | A | $0.000275 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 117 | | $0.000322 |

Uploaded 1.4 GiB (segments 1.4 GiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 1.2 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 14,313.6 µs |
| read | 193 | 3,884.3 µs |
| write | 13,147 | 1,652.7 µs |
| free | 13,522 | 22.0 µs |
| map | 9,175 | 67.4 µs |
| unmap | 5,784 | 82.7 µs |
| commit | 28 | 450,924.0 µs |
| root | 1 | 1.4 µs |
| info | 4 | 0.8 µs |
| window | 1 | 38.7 µs |
| transfer | 1 | 7.5 µs |

Commits: 839, 37,913.3 µs each: writing the open extents' 2,349.6 KiB 1,149.0 µs, committing the store 72.2 µs, sealing the windows' mappings 668.4 µs, waiting for object storage 36,023.3 µs.

Segments closed and framed: 50, 371.9 ms each.

Every workload is within 2× of ext4.
