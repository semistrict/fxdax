Host: linux. Iterations: fxdax-dax 3.

| Workload | Unit | fxdax-dax |
|---|---|---|
| seq-write | MiB/s | 35.3 (34.6–41.6) |
| rand-read-cold | IOPS | 33,574 (30,118–35,930) |
| seq-read-cold | MiB/s | 848 (823–880) |
| seq-read-warm | MiB/s | 10,456 (10,393–10,522) |
| rand-read-warm | IOPS | 852,234 (846,283–856,869) |
| rand-write | IOPS | 4,572 (4,552–4,572) |
| mmap-rand-read | IOPS | 1,109,428 (1,108,914–1,111,258) |
| mmap-rand-write | IOPS | 4,573 (4,572–5,471) |
| fsync-append | IOPS | 398 (397–398) |

- fxdax-dax: mtime-ahead-of-guest-ms 171 median (171–171)

