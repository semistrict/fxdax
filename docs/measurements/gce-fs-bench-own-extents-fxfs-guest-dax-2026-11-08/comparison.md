Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 2.52 (2.52–2.52) |
| pgbench-tps | tps | 2,042 (2,042–2,042) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -1.64 median (-1.64–-1.64)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 33 | B | $0.000013 |
| get-range | 98 | B | $0.000039 |
| head | 0 | B | $0.000000 |
| list | 4 | A | $0.000020 |
| create | 59 | A | $0.000295 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 197 | | $0.000377 |

Uploaded 47.2 MiB (segments 45.1 MiB, extents 2.1 MiB, checkpoints 0 B, moves 0 B, other 54 B); downloaded 4.7 MiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,230,002.6 µs |
| read | 478 | 6,350.1 µs |
| write | 72,836 | 29.9 µs |
| free | 27,663 | 3.8 µs |
| commit | 70 | 7,519.8 µs |
| root | 1 | 1.1 µs |
| info | 4 | 1.9 µs |
| window | 1 | 115.8 µs |
| transfer | 1 | 15.6 µs |
| free_many | 315 | 118.6 µs |

Commits: 31,849, 50.7 µs each: committing the store 11.3 µs, waiting for object storage 39.2 µs.

Segments closed and framed: 34, 24.6 ms each.

