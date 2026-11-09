Host: linux. VMM: firecracker. Iterations: fxfs-guest 1, fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest | fxfs-guest-dax |
|---|---|---|---|
| create-fsync | ops/s | 2,476 (2,476–2,476) | 2,781 (2,781–2,781) |

- fxfs-guest: mtime-ahead-of-guest-ms -2.07 median (-2.07–-2.07)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.74 median (-3.74–-3.74)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 22 | B | $0.000009 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 8 | A | $0.000040 |
| create | 20 | A | $0.000100 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 55 | | $0.000160 |

Uploaded 72.3 KiB (segments 70.1 KiB, extents 0 B, checkpoints 2.0 KiB, moves 0 B, other 96 B); downloaded 360 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 24,218.3 µs |
| write | 822 | 42.8 µs |
| commit | 16 | 3,149.7 µs |
| root | 2 | 4.7 µs |
| info | 8 | 3.6 µs |
| window | 1 | 111.2 µs |
| transfer | 2 | 11.3 µs |
| free_many | 2 | 135.3 µs |

Commits: 418, 186.7 µs each: committing the store 10.2 µs, waiting for object storage 176.2 µs.

Segments closed and framed: 10, 1.0 ms each.

