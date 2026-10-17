Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.19 (3.15–3.25) | 3.96 (3.79–4.28) | 1.24× |
| pgbench-tps | tps | 1,751 (1,737–1,797) | 1,466 (1,458–1,470) | 1.19× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -3.21 median (-5.84–-2.31)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.76 median (-7.00–-3.64)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 105 | B | $0.000042 |
| get-range | 33 | B | $0.000013 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 116 | A | $0.000580 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 260 | | $0.000660 |

Uploaded 123.4 MiB (segments 123.4 MiB, extents 0 B, checkpoints 18.4 KiB, moves 0 B, other 31 B); downloaded 1.6 KiB.

Every workload is within 2× of ext4.
