Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 65.8 (65.6–70.3) |
| rand-read-cold | IOPS | 151,704 (109,960–184,090) |
| seq-read-cold | MiB/s | 1,045 (709–1,333) |
| seq-read-warm | MiB/s | 10,850 (10,782–11,209) |
| rand-read-warm | IOPS | 793,192 (776,339–804,690) |
| rand-write | IOPS | 11,817 (11,815–11,819) |
| mmap-rand-read | IOPS | 1,104,888 (1,099,581–1,137,372) |
| mmap-rand-write | IOPS | 11,815 (11,814–11,824) |
| fsync-append | IOPS | 399 (388–400) |
| create | ops/s | 26,271 (25,381–26,529) |
| stat-cold | ops/s | 290,454 (175,389–297,101) |
| stat-warm | ops/s | 1,214,331 (866,941–1,427,579) |
| lookup-missing | ops/s | 351,207 (293,385–457,869) |
| readdir | ops/s | 2,065,811 (1,111,023–2,086,102) |
| read-small-cold | ops/s | 112,758 (110,535–115,808) |
| chmod | ops/s | 1,172,374 (1,119,094–1,229,761) |
| setxattr | ops/s | 612,672 (586,028–629,909) |
| link | ops/s | 92,741 (90,135–97,943) |
| rename | ops/s | 73,010 (68,964–73,812) |
| rename-across | ops/s | 61,727 (58,272–67,766) |
| unlink | ops/s | 166,712 (154,487–246,712) |
| create-fsync | ops/s | 1,727 (1,055–1,733) |
| mkdir-tree | ops/s | 23,919 (17,094–27,561) |
| find-cold | ops/s | 218,710 (181,219–224,330) |
| rm-tree | ops/s | 62,583 (59,249–65,299) |
| large-dir-create | ops/s | 77,383 (59,254–88,461) |
| large-dir-stat-cold | ops/s | 256,771 (249,775–265,348) |
| large-dir-unlink | ops/s | 206,322 (170,850–214,159) |
| create-parallel | ops/s | 110,789 (97,035–111,278) |
| stat-parallel | ops/s | 478,143 (459,998–524,112) |
| unlink-parallel | ops/s | 174,339 (172,746–179,746) |
| create-shared-dir | ops/s | 80,579 (80,039–82,128) |
| unlink-shared-dir | ops/s | 148,973 (145,297–160,506) |
| pgbench-init | s | 3.96 (3.85–4.01) |
| pgbench-tps | tps | 1,591 (1,590–1,595) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.45 median (-5.86–-3.61)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 243 | B | $0.000097 |
| get-range | 66 | B | $0.000026 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 245 | A | $0.001225 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 560 | | $0.001374 |

Uploaded 825.4 MiB (segments 825.4 MiB, extents 0 B, checkpoints 31.8 KiB, moves 0 B, other 39 B); downloaded 5.6 KiB.

