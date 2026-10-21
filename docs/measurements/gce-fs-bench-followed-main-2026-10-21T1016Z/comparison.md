Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3, fxfs-guest-dax-followed 3, fxfs-guest-dax-followed-idle 3.

| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed-idle | fxfs-guest-dax-followed vs fxfs-guest-dax | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 288 (283–288) | 224 (224–283) | 224 (222–224) | 1.28× | 1.28× |
| rand-read-cold | IOPS | 136,533 (111,456–165,495) | 176,172 (174,298–190,512) | 215,579 (207,392–215,579) | 0.77× | 0.63× |
| seq-read-cold | MiB/s | 931 (705–1,148) | 1,354 (1,208–1,354) | 1,610 (1,561–1,610) | 0.69× | 0.58× |
| seq-read-warm | MiB/s | 11,119 (10,917–11,310) | 10,583 (9,038–11,034) | 10,445 (10,170–10,846) | 1.05× | 1.06× |
| rand-read-warm | IOPS | 801,496 (796,339–804,708) | 763,761 (750,502–781,826) | 784,073 (780,944–802,352) | 1.05× | 1.02× |
| rand-write | IOPS | 18,481 (18,460–18,483) | 18,468 (18,445–18,475) | 18,472 (14,416–18,482) | 1.00× | 1.00× |
| mmap-rand-read | IOPS | 1,146,112 (1,113,569–1,155,430) | 964,816 (962,009–1,062,774) | 1,109,157 (1,068,650–1,141,976) | 1.19× | 1.03× |
| mmap-rand-write | IOPS | 18,476 (18,475–18,483) | 14,394 (13,180–18,443) | 18,469 (18,467–18,478) | 1.28× | 1.00× |
| fsync-append | IOPS | 402 (402–642) | 400 (395–401) | 402 (402–402) | 1.00× | 1.00× |
| pgbench-init | s | 2.99 (2.87–3.11) | 3.56 (3.54–3.60) | 3.41 (3.37–3.49) | 1.19× | 1.14× |
| pgbench-tps | tps | 1,649 (1,622–1,651) | 1,293 (1,291–1,304) | 1,155 (1,115–1,156) | 1.28× | 1.43× |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.45 median (-6.55–-2.02)
- fxfs-guest-dax-followed: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower1-raced-reads 602 median (602–602)
- fxfs-guest-dax-followed: follower1-reads 157,851 median (157,851–157,851)
- fxfs-guest-dax-followed: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower2-raced-reads 602 median (602–602)
- fxfs-guest-dax-followed: follower2-reads 157,497 median (157,497–157,497)
- fxfs-guest-dax-followed: mtime-ahead-of-guest-ms -2.82 median (-3.86–-1.76)
- fxfs-guest-dax-followed-idle: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: mtime-ahead-of-guest-ms -6.50 median (-6.51–-4.03)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 2,145 | B | $0.000858 |
| get-range | 4,861 | B | $0.001944 |
| head | 0 | B | $0.000000 |
| list | 9 | A | $0.000045 |
| create | 2,176 | A | $0.010880 |
| update | 6 | A | $0.000030 |
| overwrite | 0 | A | $0.000000 |
| delete | 3 | free | $0.000000 |
| all | 9,200 | | $0.013757 |

Uploaded 2.4 GiB (segments 2.4 GiB, extents 0 B, checkpoints 665.5 KiB, moves 0 B, other 117 B); downloaded 49.9 KiB.

Followers: at most 2 at once, at most 409 commits and 2,421 ms behind their writer, keeping at most 435.8 MiB of its frees.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 7 | 3,559.3 µs |
| read | 328,937 | 383.2 µs |
| write | 205,216 | 487.0 µs |
| free | 372,423 | 62.2 µs |
| map | 706,131 | 76.4 µs |
| unmap | 178,991 | 123.9 µs |
| commit | 400 | 19,609.9 µs |
| root | 15 | 0.7 µs |
| info | 28 | 0.8 µs |
| window | 7 | 31.4 µs |
| fault | 251,758 | 121.1 µs |
| advance | 40,598 | 2.0 µs |
| let_go | 40,594 | 0.6 µs |
| pin | 3,080 | 0.8 µs |
| unpin | 3,080 | 1.1 µs |
| transfer | 7 | 12.1 µs |

Commits: 175,214, 511.9 µs each: writing the open extents' 49.7 KiB 79.5 µs, committing the store 31.5 µs, sealing the windows' mappings 71.9 µs, waiting for object storage 328.6 µs.

Segments closed and framed: 2,127, 26.9 ms each.

Every workload is within 2× of ext4.
