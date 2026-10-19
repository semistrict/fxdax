Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 70.3 (70.1–70.3) |
| rand-read-cold | IOPS | 163,840 (110,703–210,051) |
| seq-read-cold | MiB/s | 1,128 (715–1,506) |
| seq-read-warm | MiB/s | 11,370 (11,029–11,584) |
| rand-read-warm | IOPS | 812,811 (811,826–813,113) |
| rand-write | IOPS | 11,816 (11,812–11,820) |
| mmap-rand-read | IOPS | 1,117,218 (1,111,693–1,138,295) |
| mmap-rand-write | IOPS | 11,820 (7,309–11,821) |
| fsync-append | IOPS | 402 (223–402) |
| create | ops/s | 14,742 (5,637–24,516) |
| stat-cold | ops/s | 222,926 (181,670–296,061) |
| stat-warm | ops/s | 1,389,471 (1,369,201–1,444,166) |
| lookup-missing | ops/s | 453,226 (450,246–476,622) |
| readdir | ops/s | 2,057,935 (2,000,628–2,123,636) |
| read-small-cold | ops/s | 114,359 (113,081–114,849) |
| chmod | ops/s | 1,321,203 (1,273,865–1,340,759) |
| setxattr | ops/s | 559,923 (267,719–642,261) |
| link | ops/s | 97,445 (96,046–97,779) |
| rename | ops/s | 80,928 (80,925–81,152) |
| rename-across | ops/s | 71,680 (70,278–74,816) |
| unlink | ops/s | 196,219 (154,220–206,907) |
| create-fsync | ops/s | 1,503 (772–1,593) |
| mkdir-tree | ops/s | 25,413 (10,392–27,579) |
| find-cold | ops/s | 216,593 (165,393–222,963) |
| rm-tree | ops/s | 64,837 (60,016–66,789) |
| large-dir-create | ops/s | 82,848 (63,280–89,122) |
| large-dir-stat-cold | ops/s | 285,159 (257,758–286,427) |
| large-dir-unlink | ops/s | 219,477 (176,642–225,694) |
| create-parallel | ops/s | 118,202 (111,414–120,113) |
| stat-parallel | ops/s | 605,866 (531,350–607,147) |
| unlink-parallel | ops/s | 187,537 (174,633–199,556) |
| create-shared-dir | ops/s | 85,704 (79,246–88,871) |
| unlink-shared-dir | ops/s | 195,543 (182,266–201,749) |
| untar | s | 0.02 (0.02–0.08) |
| make | s | 4.66 (4.62–4.70) |
| build-total | s | 4.71 (4.64–4.79) |

- build failed in 1 of fxfs-guest-dax's iterations, which have no measurement.
- fxfs-guest-dax: mtime-ahead-of-guest-ms -7.42 median (-7.46–-4.73)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 41 | B | $0.000016 |
| get-range | 3 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 49 | A | $0.000245 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 99 | | $0.000288 |

Uploaded 1.2 GiB (segments 1.2 GiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 870 B.

