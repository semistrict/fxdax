Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.21 (3.21–3.54) | 3.25 (3.20–3.26) | 1.01× |
| pgbench-tps | tps | 1,881 (1,825–1,902) | 1,366 (1,366–1,375) | 1.38× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -2.73 median (-4.24–-1.87)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.26 median (-7.49–-5.16)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 100 | B | $0.000040 |
| get-range | 22 | B | $0.000009 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 122 | A | $0.000610 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 250 | | $0.000684 |

Uploaded 122.0 MiB (segments 122.0 MiB, extents 0 B, checkpoints 14.6 KiB, moves 0 B, other 39 B); downloaded 2.2 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,322,851.2 µs |
| read | 1,018 | 492.5 µs |
| write | 68,360 | 119.9 µs |
| free | 125,985 | 50.6 µs |
| map | 99,788 | 33.6 µs |
| unmap | 62,423 | 32.0 µs |
| commit | 136 | 99,185.3 µs |
| root | 1 | 1.1 µs |
| info | 4 | 0.8 µs |
| window | 1 | 34.4 µs |
| transfer | 1 | 12.3 µs |

Commits: 61,728, 328.9 µs each: writing the open extents' 28.4 KiB 29.9 µs, committing the store 10.0 µs, sealing the windows' mappings 50.8 µs, waiting for object storage 237.8 µs.

Segments closed and framed: 116, 59.7 ms each.

Every workload is within 2× of ext4.
