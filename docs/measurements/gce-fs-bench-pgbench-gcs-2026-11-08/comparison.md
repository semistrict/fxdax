Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.29 (3.27–3.30) | 1.70 (1.62–2.50) | 0.52× |
| pgbench-tps | tps | 1,826 (1,742–1,828) | 2,174 (2,135–2,179) | 0.84× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -5.35 median (-5.95–-3.28)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.84 median (-6.27–-3.38)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 85 | B | $0.000034 |
| get-range | 242 | B | $0.000097 |
| head | 0 | B | $0.000000 |
| list | 5 | A | $0.000025 |
| create | 133 | A | $0.000665 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 468 | | $0.000831 |

Uploaded 156.5 MiB (segments 153.0 MiB, extents 3.5 MiB, checkpoints 83.2 KiB, moves 0 B, other 54 B); downloaded 13.2 MiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 3,518,250.1 µs |
| read | 1,547 | 5,834.0 µs |
| write | 228,456 | 27.5 µs |
| free | 86,929 | 3.6 µs |
| commit | 208 | 5,070.6 µs |
| root | 1 | 1.2 µs |
| info | 4 | 2.1 µs |
| window | 1 | 126.1 µs |
| transfer | 1 | 9.4 µs |
| free_many | 1,034 | 127.1 µs |

Commits: 100,239, 40.4 µs each: committing the store 11.3 µs, waiting for object storage 28.9 µs.

Segments closed and framed: 104, 25.3 ms each.

Every workload is within 2× of ext4.
