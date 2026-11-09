| Case | Load | Run 1 tps | Run 2 tps | Run 3 tps | Run 4 tps | Run 5 tps | Run 6 tps |
|---|---|---|---|---|---|---|---|
| fxfs-guest-pgbench | 19.16 s | 1,549 | 1,601 | 1,624 | 1,602 | 1,641 | – |
| fxfs-guest-pgbench-dax | 19.37 s | 1,565 | 1,587 | 1,613 | 1,585 | 1,639 | – |
| fxfs-guest-pgbench-dax-forked | 19.90 s | 1,307 | 1,362 | 1,577 | 1,395 | 1,546 | – |
| fxfs-guest-pgbench-forked | 19.59 s | 1,355 | 1,545 | 1,594 | 1,625 | 1,634 | – |
| pgbench-ext4-blk-unsafe | 14.38 s | 2,604 | 3,386 | 3,080 | 3,258 | 3,329 | – |
| pgbench-ext4-blk-writeback | 18.62 s | 1,260 | 1,249 | 1,248 | 1,276 | 1,305 | – |
| pgbench-ext4-dax-pmem | 23.23 s | 1,235 | 1,209 | 1,224 | 1,284 | 1,276 | – |
| fxfs-guest-pgbench-dax-density | 19.58 s | 1,235 | 1,363 | 1,321 | 1,345 | 1,340 | 1,301 |
| fxfs-guest-pgbench-density | 19.32 s | 1,359 | 1,349 | 1,357 | 1,352 | 1,351 | 1,313 |

## fxfs-guest-pgbench

Load (pgbench -i): 19.16 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.71 s, vacuum 0.59 s, primary keys 4.85 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,549 | 2.58 ms | 37% | 41% | 22% | 1% | write 2,670 (53), free 322 (7), read 20 (222), free_many 14 (120), commit 2 (48) | 1,096 (9 us) |
| 2 | 1,601 | 2.50 ms | 39% | 42% | 18% | 1% | write 2,798 (45), free 421 (7), free_many 265 (19), read 20 (255), commit 2 (37) | 1,110 (13 us) |
| 3 | 1,624 | 2.46 ms | 39% | 42% | 18% | 1% | write 2,801 (47), free 462 (7), free_many 141 (29), read 21 (240), commit 2 (34) | 1,135 (13 us) |
| 4 | 1,602 | 2.50 ms | 39% | 42% | 19% | 1% | write 2,756 (49), free 414 (8), free_many 171 (26), read 20 (229), commit 2 (37) | 1,119 (14 us) |
| 5 | 1,641 | 2.44 ms | 40% | 42% | 18% | 0% | write 2,826 (43), free 458 (7), free_many 126 (28), read 21 (239), commit 2 (44) | 1,147 (14 us) |

## fxfs-guest-pgbench-dax

Load (pgbench -i): 19.37 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.63 s, vacuum 0.57 s, primary keys 5.16 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,565 | 2.56 ms | 38% | 41% | 21% | 1% | write 2,696 (55), free 330 (7), read 20 (243), free_many 14 (121), commit 2 (40) | 1,109 (10 us) |
| 2 | 1,587 | 2.52 ms | 39% | 43% | 18% | 1% | write 2,779 (47), free 421 (8), free_many 266 (19), read 21 (250), commit 2 (43) | 1,108 (12 us) |
| 3 | 1,613 | 2.48 ms | 39% | 42% | 18% | 1% | write 2,783 (47), free 458 (7), free_many 126 (31), read 21 (285), commit 2 (34) | 1,130 (13 us) |
| 4 | 1,585 | 2.52 ms | 38% | 42% | 19% | 1% | write 2,718 (50), free 424 (8), free_many 175 (25), read 20 (304), commit 2 (40) | 1,107 (13 us) |
| 5 | 1,639 | 2.44 ms | 40% | 42% | 18% | 0% | write 2,819 (43), free 448 (8), free_many 121 (32), read 22 (236), commit 2 (40) | 1,146 (14 us) |

## fxfs-guest-pgbench-dax-forked

Load (pgbench -i): 19.90 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.57 s, vacuum 0.47 s, primary keys 5.85 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,307 | 3.06 ms | 35% | 44% | 21% | 1% | – | – |
| 2 | 1,362 | 2.94 ms | 34% | 35% | 30% | 1% | write 1,930 (70), map 511 (83), free 385 (18), private 290 (241), free_many 184 (59), read 17 (293), commit 2 (754), unmap 0 (61), fault 0 (47) | 924 (10 us) |
| 3 | 1,577 | 2.54 ms | 38% | 38% | 23% | 1% | write 2,268 (60), free 396 (9), map 227 (91), private 120 (130), free_many 72 (53), read 20 (309), commit 2 (675), fault 1 (405), unmap 0 (295) | 1,098 (13 us) |
| 4 | 1,395 | 2.87 ms | 34% | 36% | 30% | 1% | write 1,965 (83), map 660 (88), free 380 (21), free_many 168 (62), private 86 (692), fault 80 (75), read 18 (244), commit 2 (121) | 942 (12 us) |
| 5 | 1,546 | 2.59 ms | 37% | 39% | 24% | 1% | write 2,230 (70), free 446 (11), free_many 261 (30), map 210 (89), private 29 (722), read 21 (277), fault 8 (55), commit 2 (54), unmap 0 (83) | 1,077 (13 us) |

## fxfs-guest-pgbench-forked

Load (pgbench -i): 19.59 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.79 s, vacuum 0.46 s, primary keys 5.33 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,355 | 2.95 ms | 33% | 49% | 17% | 1% | – | – |
| 2 | 1,545 | 2.59 ms | 38% | 42% | 19% | 1% | write 2,752 (52), free 432 (7), free_many 226 (20), read 20 (236), commit 2 (40) | 1,075 (12 us) |
| 3 | 1,594 | 2.51 ms | 39% | 42% | 19% | 1% | write 2,725 (46), free 391 (7), free_many 81 (38), read 20 (234), commit 2 (38) | 1,108 (12 us) |
| 4 | 1,625 | 2.46 ms | 39% | 42% | 19% | 1% | write 2,664 (48), free 450 (8), free_many 219 (22), read 20 (240), commit 2 (53) | 1,094 (13 us) |
| 5 | 1,634 | 2.45 ms | 39% | 42% | 18% | 1% | write 2,797 (44), free 466 (7), free_many 287 (21), read 21 (224), commit 2 (35) | 1,139 (14 us) |

## pgbench-ext4-blk-unsafe

Load (pgbench -i): 14.38 s: drop tables 0.00 s, create tables 0.00 s, client-side generate 10.20 s, vacuum 0.37 s, primary keys 3.80 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 2,604 | 1.54 ms | 46% | 44% | 11% | 0% | – | – |
| 2 | 3,386 | 1.18 ms | 57% | 39% | 4% | 0% | – | – |
| 3 | 3,080 | 1.30 ms | 53% | 42% | 5% | 0% | – | – |
| 4 | 3,258 | 1.23 ms | 56% | 39% | 5% | 0% | – | – |
| 5 | 3,329 | 1.20 ms | 56% | 40% | 4% | 0% | – | – |

## pgbench-ext4-blk-writeback

Load (pgbench -i): 18.62 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.41 s, vacuum 0.55 s, primary keys 4.65 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,260 | 3.17 ms | 25% | 20% | 55% | 0% | – | – |
| 2 | 1,249 | 3.20 ms | 25% | 18% | 57% | 0% | – | – |
| 3 | 1,248 | 3.21 ms | 25% | 18% | 57% | 0% | – | – |
| 4 | 1,276 | 3.13 ms | 25% | 18% | 56% | 0% | – | – |
| 5 | 1,305 | 3.06 ms | 26% | 19% | 55% | 0% | – | – |

## pgbench-ext4-dax-pmem

Load (pgbench -i): 23.23 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 15.43 s, vacuum 1.46 s, primary keys 6.33 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,235 | 3.24 ms | 29% | 26% | 45% | 0% | – | – |
| 2 | 1,209 | 3.31 ms | 31% | 27% | 43% | 0% | – | – |
| 3 | 1,224 | 3.27 ms | 29% | 26% | 45% | 0% | – | – |
| 4 | 1,284 | 3.12 ms | 31% | 27% | 42% | 0% | – | – |
| 5 | 1,276 | 3.14 ms | 30% | 27% | 43% | 0% | – | – |

## fxfs-guest-pgbench-dax-density

Load (pgbench -i): 19.58 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.92 s, vacuum 0.43 s, primary keys 5.22 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,235 | 3.24 ms | 35% | 44% | 21% | 1% | – | – |
| 2 | 1,363 | 2.93 ms | 35% | 38% | 26% | 1% | write 2,013 (73), private 342 (61), free 342 (10), map 306 (90), free_many 20 (128), read 18 (243), commit 2 (78), unmap 0 (69), fault 0 (494) | 957 (10 us) |
| 3 | 1,321 | 3.03 ms | 36% | 39% | 24% | 1% | – | – |
| 4 | 1,345 | 2.97 ms | 35% | 35% | 29% | 1% | write 1,959 (74), map 500 (92), private 382 (62), free 324 (10), free_many 20 (135), read 18 (244), commit 2 (79), fault 0 (258), unmap 0 (69) | 925 (10 us) |
| 5 | 1,340 | 2.98 ms | 37% | 40% | 22% | 1% | – | – |
| 6 | 1,301 | 3.08 ms | 34% | 34% | 30% | 1% | write 1,959 (74), map 500 (92), private 382 (62), free 324 (10), free_many 20 (135), read 18 (244), commit 2 (79), fault 0 (258), unmap 0 (69) | 925 (10 us) |

## fxfs-guest-pgbench-density

Load (pgbench -i): 19.32 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.88 s, vacuum 0.63 s, primary keys 4.80 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,359 | 2.94 ms | 34% | 46% | 18% | 1% | – | – |
| 2 | 1,349 | 2.96 ms | 35% | 44% | 20% | 1% | write 2,423 (42), read 1,397 (119), free 390 (8), free_many 194 (23), commit 2 (40) | 961 (11 us) |
| 3 | 1,357 | 2.95 ms | 34% | 47% | 18% | 1% | – | – |
| 4 | 1,352 | 2.96 ms | 35% | 44% | 20% | 1% | write 2,423 (42), read 1,397 (119), free 390 (8), free_many 194 (23), commit 2 (40) | 961 (11 us) |
| 5 | 1,351 | 2.96 ms | 35% | 46% | 18% | 1% | – | – |
| 6 | 1,313 | 3.05 ms | 35% | 43% | 20% | 2% | write 2,423 (42), read 1,397 (119), free 390 (8), free_many 194 (23), commit 2 (40) | 961 (11 us) |
