Host: linux. Iterations: fxdax-dax 1, fxdax-nodax 1.

| Workload | Unit | fxdax-dax | fxdax-nodax |
|---|---|---|---|
| seq-write | MiB/s | 43.3 (43.3–43.3) | 70.0 (70.0–70.0) |
| rand-read-cold | IOPS | 8,924 (8,924–8,924) | 8,982 (8,982–8,982) |
| seq-read-cold | MiB/s | 173 (173–173) | 321 (321–321) |
| seq-read-warm | MiB/s | 10,438 (10,438–10,438) | 7,610 (7,610–7,610) |
| rand-read-warm | IOPS | 854,586 (854,586–854,586) | 811,714 (811,714–811,714) |
| rand-write | IOPS | 4,551 (4,551–4,551) | 4,559 (4,559–4,559) |
| mmap-rand-read | IOPS | 13,958 (13,958–13,958) | 8,963 (8,963–8,963) |
| mmap-rand-write | IOPS | 1,546 (1,546–1,546) | 5,468 (5,468–5,468) |
| fsync-append | IOPS | 395 (395–395) | 398 (398–398) |
| create | ops/s | 1,614 (1,614–1,614) | 1,627 (1,627–1,627) |
| stat-cold | ops/s | 4,358 (4,358–4,358) | 4,731 (4,731–4,731) |
| readdir | ops/s | 369,626 (369,626–369,626) | 324,621 (324,621–324,621) |
| unlink | ops/s | 3,543 (3,543–3,543) | 3,614 (3,614–3,614) |

- fxdax-dax: mtime-ahead-of-guest-ms 170 median (170–170)
- fxdax-nodax: mtime-ahead-of-guest-ms 170 median (170–170)

