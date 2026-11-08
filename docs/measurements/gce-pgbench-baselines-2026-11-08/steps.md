| Case | Load | Run 1 tps | Run 2 tps |
|---|---|---|---|
| fxfs-guest-pgbench | 13.46 s | 2,011 | 2,037 |
| fxfs-guest-pgbench-dax | 13.45 s | 2,004 | 2,047 |
| pgbench-ext4-blk-unsafe | 10.87 s | 3,609 | 3,820 |
| pgbench-ext4-blk-writeback | 15.16 s | 1,536 | 1,534 |
| pgbench-ext4-dax-pmem | 17.48 s | 1,263 | 1,163 |

## fxfs-guest-pgbench

Load (pgbench -i): 13.46 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 9.44 s, vacuum 0.25 s, primary keys 3.76 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 2,011 | 1.99 ms | 40% | 41% | 19% | 0% | write 3,340 (34), free 455 (4), read 24 (236), free_many 20 (84), commit 3 (25) | 1,370 (5 us) |
| 2 | 2,037 | 1.96 ms | 40% | 42% | 18% | 0% | write 3,366 (31), free 478 (4), free_many 295 (14), read 25 (173), commit 3 (50) | 1,364 (6 us) |

## fxfs-guest-pgbench-dax

Load (pgbench -i): 13.45 s: drop tables 0.00 s, create tables 0.00 s, client-side generate 9.44 s, vacuum 0.43 s, primary keys 3.59 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 2,004 | 2.00 ms | 40% | 41% | 18% | 0% | write 3,334 (33), free 458 (4), read 24 (190), free_many 20 (82), commit 3 (20) | 1,370 (5 us) |
| 2 | 2,047 | 1.95 ms | 40% | 42% | 17% | 0% | write 3,312 (38), free 475 (4), free_many 281 (15), read 25 (182), commit 3 (128) | 1,332 (20 us) |

## pgbench-ext4-blk-unsafe

Load (pgbench -i): 10.87 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 7.57 s, vacuum 0.24 s, primary keys 3.05 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 3,609 | 1.11 ms | 47% | 44% | 9% | 0% | – | – |
| 2 | 3,820 | 1.05 ms | 49% | 43% | 9% | 0% | – | – |

## pgbench-ext4-blk-writeback

Load (pgbench -i): 15.16 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 9.82 s, vacuum 0.23 s, primary keys 5.09 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,536 | 2.60 ms | 24% | 21% | 55% | 0% | – | – |
| 2 | 1,534 | 2.61 ms | 24% | 20% | 57% | 0% | – | – |

## pgbench-ext4-dax-pmem

Load (pgbench -i): 17.48 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 11.54 s, vacuum 0.97 s, primary keys 4.97 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,263 | 3.17 ms | 23% | 19% | 57% | 0% | – | – |
| 2 | 1,163 | 3.44 ms | 22% | 18% | 60% | 0% | – | – |
