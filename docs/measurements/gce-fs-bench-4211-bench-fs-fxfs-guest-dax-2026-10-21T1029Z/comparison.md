Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 3.08 (3.01–3.15) |
| pgbench-tps | tps | 1,638 (1,594–1,653) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.80 median (-6.79–-4.66)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 125 | B | $0.000050 |
| get-range | 24 | B | $0.000010 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 125 | A | $0.000625 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 280 | | $0.000710 |

Uploaded 128.3 MiB (segments 128.3 MiB, extents 0 B, checkpoints 16.0 KiB, moves 0 B, other 39 B); downloaded 2.8 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 13,711.4 µs |
| read | 1,181 | 544.9 µs |
| write | 78,773 | 111.1 µs |
| free | 147,599 | 45.8 µs |
| map | 126,312 | 33.7 µs |
| unmap | 63,978 | 40.2 µs |
| commit | 152 | 19,337.6 µs |
| root | 1 | 1.4 µs |
| info | 4 | 1.0 µs |
| window | 1 | 40.3 µs |
| transfer | 1 | 8.6 µs |

Commits: 72,041, 136.4 µs each: writing the open extents' 25.6 KiB 30.0 µs, committing the store 10.2 µs, sealing the windows' mappings 52.2 µs, waiting for object storage 43.6 µs.

Segments closed and framed: 119, 55.6 ms each.

