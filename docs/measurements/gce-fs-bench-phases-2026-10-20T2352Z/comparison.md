Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.10 (4.10–4.10) |
| pgbench-tps | tps | 1,325 (1,325–1,325) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.01 median (-3.01–-3.01)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 37 | B | $0.000015 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 44 | A | $0.000220 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 89 | | $0.000261 |

Uploaded 39.7 MiB (segments 39.7 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 774 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,347,543.1 µs |
| read | 308 | 585.6 µs |
| write | 21,450 | 198.3 µs |
| free | 38,275 | 48.7 µs |
| map | 21,524 | 32.2 µs |
| unmap | 19,529 | 17.8 µs |
| commit | 44 | 105,387.8 µs |
| root | 1 | 1.2 µs |
| info | 4 | 0.9 µs |
| window | 1 | 27.3 µs |
| transfer | 1 | 10.6 µs |

Commits: 19,279, 438.8 µs each: committing the store 148.4 µs, sealing the windows' mappings 38.1 µs, waiting for object storage 252.0 µs.

