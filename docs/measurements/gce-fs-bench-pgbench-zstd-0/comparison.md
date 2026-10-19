Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 5.00 (4.95–5.05) |
| pgbench-tps | tps | 1,262 (1,250–1,268) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -1.97 median (-4.71–-0.97)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 91 | B | $0.000036 |
| get-range | 27 | B | $0.000011 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 121 | A | $0.000605 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 245 | | $0.000677 |

Uploaded 1.9 GiB (segments 1.9 GiB, extents 0 B, checkpoints 139.5 KiB, moves 0 B, other 39 B); downloaded 2.0 KiB.

