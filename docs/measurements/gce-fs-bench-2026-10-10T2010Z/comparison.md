Host: linux. Iterations: fxdax-dax 3, fxdax-nodax 3.

| Workload | Unit | fxdax-dax | fxdax-nodax |
|---|---|---|---|
| seq-write | MiB/s | 34.6 (34.6–41.5) | 55.1 (52.3–70.0) |
| rand-read-cold | IOPS | 27,864 (27,676–28,055) | 9,638 (9,309–10,639) |
| seq-read-cold | MiB/s | 795 (788–810) | 1,652 (390–1,673) |
| seq-read-warm | MiB/s | 10,554 (10,460–10,590) | 7,571 (7,481–7,581) |
| rand-read-warm | IOPS | 857,693 (855,545–858,910) | 801,366 (797,421–813,926) |
| rand-write | IOPS | 4,560 (4,548–4,574) | 4,579 (4,552–4,581) |
| mmap-rand-read | IOPS | 1,110,658 (1,103,218–1,112,302) | 8,978 (8,334–9,621) |
| mmap-rand-write | IOPS | 4,573 (4,569–4,573) | 6,357 (4,578–6,360) |
| fsync-append | IOPS | 398 (397–399) | 398 (398–398) |

- fxdax-dax: mtime-ahead-of-guest-ms 171 median (171–171)
- fxdax-nodax: mtime-ahead-of-guest-ms 172 median (172–172)

