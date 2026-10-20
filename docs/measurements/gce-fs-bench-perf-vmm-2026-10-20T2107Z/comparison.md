Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.25 (4.25–4.25) |
| pgbench-tps | tps | 1,338 (1,338–1,338) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.21 median (-4.21–-4.21)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 36 | B | $0.000014 |
| get-range | 20 | B | $0.000008 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 38 | A | $0.000190 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 100 | | $0.000237 |

Uploaded 40.0 MiB (segments 40.0 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 750 B.

