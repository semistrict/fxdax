Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.30 (4.30–4.30) |
| pgbench-tps | tps | 1,395 (1,395–1,395) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.35 median (-4.35–-4.35)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 37 | B | $0.000015 |
| get-range | 24 | B | $0.000010 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 41 | A | $0.000205 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 108 | | $0.000254 |

Uploaded 40.5 MiB (segments 40.5 MiB, extents 0 B, checkpoints 11.5 KiB, moves 0 B, other 39 B); downloaded 774 B.

