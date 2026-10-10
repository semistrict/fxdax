Host: linux. Iterations: ext4-virtio-blk 1, fxdax-nodax 1.

| Workload | Unit | ext4-virtio-blk | fxdax-nodax | fxdax-nodax vs ext4 |
|---|---|---|---|---|
| seq-write | MiB/s | 96.2 (96.2–96.2) | 70.0 (70.0–70.0) | 1.37× |
| rand-read-cold | IOPS | 14,322 (14,322–14,322) | 8,359 (8,359–8,359) | 1.71× |
| seq-read-cold | MiB/s | 2,876 (2,876–2,876) | 314 (314–314) | **9.15×** |
| seq-read-warm | MiB/s | 7,527 (7,527–7,527) | 7,591 (7,591–7,591) | 0.99× |
| rand-read-warm | IOPS | 806,013 (806,013–806,013) | 816,387 (816,387–816,387) | 0.99× |
| rand-write | IOPS | 6,357 (6,357–6,357) | 4,556 (4,556–4,556) | 1.40× |
| mmap-rand-read | IOPS | 1,203,738 (1,203,738–1,203,738) | 8,405 (8,405–8,405) | **143.21×** |
| mmap-rand-write | IOPS | 6,366 (6,366–6,366) | 5,471 (5,471–5,471) | 1.16× |
| fsync-append | IOPS | 224 (224–224) | 399 (399–399) | 0.56× |

- ext4-virtio-blk: mtime-ahead-of-guest-ms -1.62 median (-1.62–-1.62)
- fxdax-nodax: mtime-ahead-of-guest-ms 169 median (169–169)

Over 2× slower than ext4:
- seq-read-cold on fxdax-nodax: 9.15×
- mmap-rand-read on fxdax-nodax: 143.21×
