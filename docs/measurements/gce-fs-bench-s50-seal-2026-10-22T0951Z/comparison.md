Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 25.9 (25.9–25.9) |
| pgbench-tps | tps | 562 (562–562) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.95 median (-5.95–-5.95)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 63 | B | $0.000025 |
| get-range | 62 | B | $0.000025 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 123 | A | $0.000615 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 254 | | $0.000690 |

Uploaded 109.5 MiB (segments 109.5 MiB, extents 0 B, checkpoints 16.2 KiB, moves 0 B, other 39 B); downloaded 1.4 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,242,118.5 µs |
| read | 187 | 679.6 µs |
| write | 12,404 | 238.1 µs |
| free | 15,526 | 29.4 µs |
| map | 67,017 | 42.0 µs |
| unmap | 5,206 | 416.6 µs |
| commit | 26 | 250,793.6 µs |
| root | 1 | 1.3 µs |
| info | 4 | 0.9 µs |
| window | 1 | 32.5 µs |
| fault | 47,600 | 182.8 µs |
| transfer | 1 | 12.2 µs |

Commits: 5,048, 1,820.9 µs each: writing the open extents' 333.7 KiB 207.5 µs, committing the store 33.2 µs, sealing the windows' mappings 235.4 µs, waiting for object storage 1,344.4 µs.

Segments closed and framed: 117, 56.1 ms each.

