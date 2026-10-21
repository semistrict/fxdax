Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.61 (4.61–4.61) |
| pgbench-tps | tps | 1,310 (1,310–1,310) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.60 median (-3.60–-3.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 36 | B | $0.000014 |
| get-range | 0 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 41 | A | $0.000205 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 83 | | $0.000244 |

Uploaded 39.7 MiB (segments 39.7 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 750 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,209,078.0 µs |
| read | 308 | 535.7 µs |
| write | 21,560 | 222.8 µs |
| free | 38,245 | 49.5 µs |
| map | 22,761 | 32.2 µs |
| unmap | 19,627 | 18.4 µs |
| commit | 42 | 111,652.1 µs |
| root | 1 | 1.8 µs |
| info | 4 | 1.1 µs |
| window | 1 | 36.4 µs |
| transfer | 1 | 10.3 µs |

Commits: 19,392, 457.8 µs each: writing the open extents' 29.5 KiB 28.1 µs, committing the store 124.8 µs, sealing the windows' mappings 40.0 µs, waiting for object storage 264.6 µs.

Segments closed, inline in commits: 36, 66.6 ms each.

