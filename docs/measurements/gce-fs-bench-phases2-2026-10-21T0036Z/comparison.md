Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.05 (4.05–4.05) |
| pgbench-tps | tps | 1,329 (1,329–1,329) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.98 median (-4.98–-4.98)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 37 | B | $0.000015 |
| get-range | 0 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 43 | A | $0.000215 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 86 | | $0.000255 |

Uploaded 39.8 MiB (segments 39.8 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 774 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 3,393,408.2 µs |
| read | 308 | 550.5 µs |
| write | 21,560 | 215.9 µs |
| free | 37,617 | 48.5 µs |
| map | 22,027 | 32.4 µs |
| unmap | 19,626 | 18.2 µs |
| commit | 44 | 96,222.7 µs |
| root | 1 | 1.3 µs |
| info | 4 | 1.0 µs |
| window | 1 | 37.1 µs |
| transfer | 1 | 10.8 µs |

Commits: 19,392, 433.4 µs each: writing the open extents' 29.6 KiB 29.4 µs, committing the store 127.1 µs, sealing the windows' mappings 39.7 µs, waiting for object storage 236.9 µs.

