Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| pgbench-init | s | 4.10 (4.07–4.23) |
| pgbench-tps | tps | 1,442 (1,439–1,479) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -7.64 median (-101.44–-3.45)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 110 | B | $0.000044 |
| get-range | 25 | B | $0.000010 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 129 | A | $0.000645 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 270 | | $0.000724 |

Uploaded 123.6 MiB (segments 123.5 MiB, extents 0 B, checkpoints 13.8 KiB, moves 0 B, other 39 B); downloaded 2.5 KiB.

