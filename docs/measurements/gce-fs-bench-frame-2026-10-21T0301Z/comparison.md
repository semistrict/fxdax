Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.19 (3.15–3.32) | 3.43 (3.29–3.47) | 1.08× |
| pgbench-tps | tps | 1,880 (1,786–1,904) | 1,361 (1,341–1,380) | 1.38× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.36 median (-4.63–-2.61)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.98 median (-126.82–-5.98)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 94 | B | $0.000038 |
| get-range | 26 | B | $0.000010 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 119 | A | $0.000595 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 245 | | $0.000668 |

Uploaded 120.8 MiB (segments 120.8 MiB, extents 0 B, checkpoints 15.9 KiB, moves 0 B, other 39 B); downloaded 2.1 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,159,271.8 µs |
| read | 975 | 590.0 µs |
| write | 65,913 | 108.4 µs |
| free | 121,533 | 47.8 µs |
| map | 77,759 | 37.0 µs |
| unmap | 60,018 | 23.0 µs |
| commit | 130 | 109,571.4 µs |
| root | 1 | 1.0 µs |
| info | 4 | 0.9 µs |
| window | 1 | 34.3 µs |
| transfer | 1 | 11.4 µs |

Commits: 59,370, 337.2 µs each: writing the open extents' 29.2 KiB 28.2 µs, committing the store 10.8 µs, sealing the windows' mappings 41.7 µs, waiting for object storage 256.1 µs.

Segments closed and framed: 113, 58.1 ms each.

Every workload is within 2× of ext4.
