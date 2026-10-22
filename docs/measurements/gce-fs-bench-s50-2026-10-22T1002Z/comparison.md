Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 16.4 (16.4–17.2) | 25.9 (25.7–27.0) | 1.58× |
| pgbench-tps | tps | 1,063 (1,062–1,068) | 477 (469–532) | **2.23×** |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -7.20 median (-7.69–-4.10)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.62 median (-7.64–-3.07)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 214 | B | $0.000086 |
| get-range | 605 | B | $0.000242 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 496 | A | $0.002480 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 1,321 | | $0.002833 |

Uploaded 361.9 MiB (segments 361.7 MiB, extents 0 B, checkpoints 202.0 KiB, moves 0 B, other 39 B); downloaded 4.9 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,250,832.1 µs |
| read | 788 | 1,097.6 µs |
| write | 47,080 | 234.8 µs |
| free | 67,871 | 27.2 µs |
| map | 234,530 | 48.5 µs |
| unmap | 16,575 | 436.9 µs |
| commit | 94 | 367,343.4 µs |
| root | 1 | 1.1 µs |
| info | 4 | 1.1 µs |
| window | 1 | 43.9 µs |
| fault | 163,442 | 248.8 µs |
| transfer | 1 | 12.1 µs |

Commits: 24,418, 1,804.4 µs each: writing the open extents' 221.5 KiB 151.8 µs, committing the store 20.2 µs, sealing the windows' mappings 193.4 µs, waiting for object storage 1,438.6 µs.

Segments closed and framed: 484, 42.8 ms each.

Over 2× slower than ext4:
- pgbench-tps on fxfs-guest-dax: 2.23× ext4-dax-pmem's
