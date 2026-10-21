Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.42 (4.42–4.42) |
| pgbench-tps | tps | 1,268 (1,268–1,268) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.63 median (-4.63–-4.63)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 35 | B | $0.000014 |
| get-range | 21 | B | $0.000008 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 38 | A | $0.000190 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 100 | | $0.000237 |

Uploaded 39.2 MiB (segments 39.2 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 726 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,214,191.1 µs |
| read | 293 | 541.9 µs |
| write | 20,546 | 235.5 µs |
| free | 36,368 | 53.5 µs |
| map | 20,540 | 38.0 µs |
| unmap | 18,631 | 20.4 µs |
| commit | 42 | 98,356.7 µs |
| root | 1 | 1.2 µs |
| info | 4 | 1.0 µs |
| window | 1 | 36.5 µs |
| transfer | 1 | 8.0 µs |

Commits: 18,405, 462.8 µs each: writing the open extents' 30.7 KiB 30.9 µs, committing the store 143.8 µs, sealing the windows' mappings 42.3 µs, waiting for object storage 245.4 µs.

