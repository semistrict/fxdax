Host: linux. Iterations: fxdax-dax 3.

| Workload | Unit | fxdax-dax |
|---|---|---|
| seq-write | MiB/s | 47.4 (45.3–59.9) |
| rand-read-cold | IOPS | 35,617 (34,420–35,617) |
| seq-read-cold | MiB/s | 889 (865–892) |
| seq-read-warm | MiB/s | 10,226 (10,197–10,656) |
| rand-read-warm | IOPS | 854,596 (854,566–866,467) |
| rand-write | IOPS | 4,540 (4,538–4,577) |
| mmap-rand-read | IOPS | 1,100,888 (1,100,494–1,121,653) |
| mmap-rand-write | IOPS | 4,582 (4,571–5,436) |
| fsync-append | IOPS | 397 (396–398) |

- fxdax-dax: mtime-ahead-of-guest-ms 171 median (171–171)

