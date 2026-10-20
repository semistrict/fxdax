Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| create | ops/s | 22,399 (22,399–22,399) |
| stat-cold | ops/s | 188,362 (188,362–188,362) |
| stat-warm | ops/s | 1,018,230 (1,018,230–1,018,230) |
| lookup-missing | ops/s | 352,586 (352,586–352,586) |
| readdir | ops/s | 1,701,786 (1,701,786–1,701,786) |
| read-small-cold | ops/s | 87,540 (87,540–87,540) |
| chmod | ops/s | 953,080 (953,080–953,080) |
| setxattr | ops/s | 448,112 (448,112–448,112) |
| link | ops/s | 86,161 (86,161–86,161) |
| rename | ops/s | 67,141 (67,141–67,141) |
| rename-across | ops/s | 52,986 (52,986–52,986) |
| unlink | ops/s | 153,885 (153,885–153,885) |
| create-fsync | ops/s | 1,784 (1,784–1,784) |
| mkdir-tree | ops/s | 26,134 (26,134–26,134) |
| find-cold | ops/s | 151,733 (151,733–151,733) |
| rm-tree | ops/s | 69,534 (69,534–69,534) |
| large-dir-create | ops/s | 57,443 (57,443–57,443) |
| large-dir-stat-cold | ops/s | 123,866 (123,866–123,866) |
| large-dir-unlink | ops/s | 209,235 (209,235–209,235) |
| create-parallel | ops/s | 32,897 (32,897–32,897) |
| stat-parallel | ops/s | 440,506 (440,506–440,506) |
| unlink-parallel | ops/s | 156,958 (156,958–156,958) |
| create-shared-dir | ops/s | 58,440 (58,440–58,440) |
| unlink-shared-dir | ops/s | 103,076 (103,076–103,076) |
| pgbench-init | s | 3.99 (3.99–3.99) |
| pgbench-tps | tps | 1,230 (1,230–1,230) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.27 median (-5.27–-5.27)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 50 | B | $0.000020 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 59 | A | $0.000295 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 117 | | $0.000341 |

Uploaded 40.9 MiB (segments 40.9 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 1.1 KiB.

