Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.57 (4.57–4.57) |
| pgbench-tps | tps | 1,179 (1,179–1,179) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -1.76 median (-1.76–-1.76)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 39 | B | $0.000016 |
| get-range | 26 | B | $0.000010 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 54 | A | $0.000270 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 125 | | $0.000321 |

Uploaded 39.2 MiB (segments 39.2 MiB, extents 0 B, checkpoints 10.5 KiB, moves 0 B, other 39 B); downloaded 822 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,343,872.6 µs |
| read | 278 | 594.9 µs |
| write | 19,414 | 246.0 µs |
| free | 34,206 | 56.4 µs |
| map | 19,394 | 48.2 µs |
| unmap | 17,513 | 21.7 µs |
| commit | 40 | 112,682.5 µs |
| root | 1 | 1.4 µs |
| info | 4 | 1.2 µs |
| window | 1 | 44.3 µs |
| transfer | 1 | 14.7 µs |

Commits: 17,293, 505.4 µs each: committing the store 176.0 µs, sealing the windows' mappings 45.4 µs, waiting for object storage 283.7 µs.

