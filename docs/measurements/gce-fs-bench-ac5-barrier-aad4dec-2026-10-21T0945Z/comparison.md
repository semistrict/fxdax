Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 70.3 (70.0–70.3) |
| rand-read-cold | IOPS | 154,566 (109,960–180,044) |
| seq-read-cold | MiB/s | 1,076 (725–1,306) |
| seq-read-warm | MiB/s | 11,003 (10,777–11,363) |
| rand-read-warm | IOPS | 785,440 (770,766–803,226) |
| rand-write | IOPS | 11,822 (11,820–14,418) |
| mmap-rand-read | IOPS | 1,104,990 (1,093,269–1,129,123) |
| mmap-rand-write | IOPS | 11,819 (11,811–11,820) |
| fsync-append | IOPS | 402 (402–655) |
| create | ops/s | 25,048 (24,874–27,163) |
| stat-cold | ops/s | 283,782 (202,534–286,736) |
| stat-warm | ops/s | 1,251,178 (1,214,362–1,343,820) |
| lookup-missing | ops/s | 383,469 (286,188–393,863) |
| readdir | ops/s | 2,035,021 (744,844–2,206,480) |
| read-small-cold | ops/s | 112,575 (105,999–119,187) |
| chmod | ops/s | 1,185,547 (1,109,288–1,194,571) |
| setxattr | ops/s | 467,293 (233,541–566,546) |
| link | ops/s | 87,728 (83,204–97,305) |
| rename | ops/s | 71,945 (66,911–74,641) |
| rename-across | ops/s | 63,855 (57,599–69,997) |
| unlink | ops/s | 168,066 (166,473–197,245) |
| create-fsync | ops/s | 1,642 (1,216–1,939) |
| mkdir-tree | ops/s | 27,707 (24,107–27,751) |
| find-cold | ops/s | 218,334 (209,718–235,838) |
| rm-tree | ops/s | 62,828 (56,823–67,660) |
| large-dir-create | ops/s | 83,498 (60,181–89,435) |
| large-dir-stat-cold | ops/s | 252,742 (251,493–257,181) |
| large-dir-unlink | ops/s | 207,128 (170,479–224,216) |
| create-parallel | ops/s | 117,093 (114,461–121,184) |
| stat-parallel | ops/s | 539,784 (536,731–605,117) |
| unlink-parallel | ops/s | 170,582 (163,315–179,456) |
| create-shared-dir | ops/s | 82,319 (82,016–88,575) |
| unlink-shared-dir | ops/s | 153,113 (149,778–156,951) |
| pgbench-init | s | 3.87 (3.77–3.88) |
| pgbench-tps | tps | 1,591 (1,587–1,609) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.89 median (-8.05–-2.69)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 233 | B | $0.000093 |
| get-range | 86 | B | $0.000034 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 236 | A | $0.001180 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 561 | | $0.001333 |

Uploaded 825.7 MiB (segments 825.7 MiB, extents 0 B, checkpoints 32.3 KiB, moves 0 B, other 39 B); downloaded 5.3 KiB.

