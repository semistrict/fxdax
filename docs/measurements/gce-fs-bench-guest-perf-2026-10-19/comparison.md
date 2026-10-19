Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 65.8 (65.6–65.8) |
| rand-read-cold | IOPS | 121,363 (96,376–184,090) |
| seq-read-cold | MiB/s | 931 (698–1,542) |
| seq-read-warm | MiB/s | 11,080 (11,043–11,219) |
| rand-read-warm | IOPS | 811,716 (809,459–816,975) |
| rand-write | IOPS | 11,818 (11,808–11,819) |
| mmap-rand-read | IOPS | 1,137,504 (1,111,416–1,146,267) |
| mmap-rand-write | IOPS | 11,819 (11,809–11,821) |
| fsync-append | IOPS | 169 (165–170) |
| create | ops/s | 23,760 (23,368–24,690) |
| stat-cold | ops/s | 261,760 (252,217–284,820) |
| stat-warm | ops/s | 1,018,494 (1,006,443–1,020,050) |
| lookup-missing | ops/s | 360,547 (304,658–402,853) |
| readdir | ops/s | 1,778,823 (1,717,216–1,809,254) |
| read-small-cold | ops/s | 105,617 (105,069–109,410) |
| chmod | ops/s | 934,131 (928,421–959,508) |
| setxattr | ops/s | 458,204 (266,508–481,436) |
| link | ops/s | 80,169 (78,581–81,927) |
| rename | ops/s | 63,668 (63,610–65,110) |
| rename-across | ops/s | 56,808 (50,423–57,204) |
| unlink | ops/s | 196,796 (171,474–269,573) |
| create-fsync | ops/s | 1,543 (696–1,797) |
| mkdir-tree | ops/s | 23,397 (23,317–24,008) |
| find-cold | ops/s | 194,331 (191,404–201,829) |
| rm-tree | ops/s | 56,220 (39,240–64,644) |
| large-dir-create | ops/s | 70,416 (51,998–73,004) |
| large-dir-stat-cold | ops/s | 218,516 (197,244–252,536) |
| large-dir-unlink | ops/s | 191,275 (152,300–196,908) |
| create-parallel | ops/s | 42,202 (41,000–46,313) |
| stat-parallel | ops/s | 176,872 (167,159–201,617) |
| unlink-parallel | ops/s | 164,460 (151,520–167,524) |
| create-shared-dir | ops/s | 39,699 (18,934–53,273) |
| unlink-shared-dir | ops/s | 163,437 (106,068–165,073) |
| untar | s | 0.33 (0.31–0.43) |
| make | s | 4.65 (4.58–4.75) |
| build-total | s | 5.08 (4.89–5.08) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.39 median (-7.06–-3.57)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 51 | B | $0.000020 |
| get-range | 4 | B | $0.000002 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 54 | A | $0.000270 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 115 | | $0.000317 |

Uploaded 703.3 MiB (segments 703.3 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 1.1 KiB.

