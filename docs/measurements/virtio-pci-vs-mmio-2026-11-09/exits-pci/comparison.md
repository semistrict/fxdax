Host: linux. VMM: firecracker. Iterations: fxfs-guest 1, fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest | fxfs-guest-dax |
|---|---|---|---|
| create-fsync | ops/s | 3,279 (3,279–3,279) | 3,441 (3,441–3,441) |

- fxfs-guest: mtime-ahead-of-guest-ms -1.62 median (-1.62–-1.62)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.33 median (-5.33–-5.33)

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

Uploaded 72.0 KiB (segments 69.9 KiB, extents 0 B, checkpoints 2.0 KiB, moves 0 B, other 96 B); downloaded 360 B.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 7,828.5 µs |
| write | 822 | 71.1 µs |
| commit | 16 | 3,095.3 µs |
| root | 2 | 1.6 µs |
| info | 8 | 2.8 µs |
| window | 1 | 111.3 µs |
| transfer | 2 | 10.7 µs |
| free_many | 2 | 119.6 µs |

Commits: 418, 242.7 µs each: committing the store 5.7 µs, waiting for object storage 236.8 µs.

Segments closed and framed: 10, 0.8 ms each.

