Host: linux. Iterations: fxdax-dax 3, fxdax-nodax 3.

| Workload | Unit | fxdax-dax | fxdax-nodax |
|---|---|---|---|
| seq-write | MiB/s | 60.2 (58.2–81.0) | 58.2 (55.0–70.0) |
| rand-read-cold | IOPS | 35,617 (34,420–35,930) | 9,246 (7,123–9,267) |
| seq-read-cold | MiB/s | 886 (880–895) | 1,684 (323–1,718) |
| seq-read-warm | MiB/s | 10,304 (10,074–10,642) | 7,539 (7,488–7,561) |
| rand-read-warm | IOPS | 856,084 (855,828–862,211) | 816,301 (812,399–818,995) |
| rand-write | IOPS | 4,567 (4,566–4,579) | 4,543 (3,567–4,580) |
| mmap-rand-read | IOPS | 1,113,655 (1,112,644–1,113,864) | 9,571 (8,145–9,599) |
| mmap-rand-write | IOPS | 4,562 (4,556–4,569) | 5,471 (5,467–6,358) |
| fsync-append | IOPS | 398 (396–398) | 399 (398–399) |

- fxdax-dax: mtime-ahead-of-guest-ms 170 median (170–170)
- fxdax-nodax: mtime-ahead-of-guest-ms 169 median (169–169)

