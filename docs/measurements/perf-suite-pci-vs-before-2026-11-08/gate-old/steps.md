| Case | Load | Run 1 tps | Run 2 tps | Run 3 tps | Run 4 tps | Run 5 tps | Run 6 tps |
|---|---|---|---|---|---|---|---|
| fxfs-guest-pgbench | 19.64 s | 1,405 | 1,382 | 1,478 | 1,450 | 1,436 | – |
| fxfs-guest-pgbench-dax | 19.82 s | 1,394 | 1,364 | 1,450 | 1,428 | 1,407 | – |
| fxfs-guest-pgbench-dax-forked | 19.61 s | 1,175 | 1,282 | 1,234 | 1,420 | 1,267 | – |
| fxfs-guest-pgbench-forked | 19.19 s | 1,189 | 1,330 | 1,432 | 1,425 | 1,417 | – |
| pgbench-ext4-blk-unsafe | 14.25 s | 2,591 | 2,725 | 2,782 | 2,782 | 2,725 | – |
| pgbench-ext4-blk-writeback | 19.31 s | 1,102 | 1,166 | 1,122 | 1,112 | 1,136 | – |
| pgbench-ext4-dax-pmem | 23.57 s | 1,175 | 1,154 | 1,196 | 1,202 | 1,211 | – |
| fxfs-guest-pgbench-dax-density | 20.55 s | 1,199 | 1,302 | 1,197 | 1,220 | 1,267 | 1,247 |
| fxfs-guest-pgbench-density | 19.41 s | 1,172 | 1,168 | 1,141 | 1,191 | 1,177 | 1,143 |

## fxfs-guest-pgbench

Load (pgbench -i): 19.64 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.89 s, vacuum 0.43 s, primary keys 5.31 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,405 | 2.85 ms | 36% | 39% | 24% | 1% | write 2,232 (60), free 284 (7), read 16 (254), free_many 12 (111), commit 2 (51) | 916 (9 us) |
| 2 | 1,382 | 2.89 ms | 36% | 41% | 22% | 1% | write 2,352 (57), free 325 (8), free_many 206 (21), read 17 (294), commit 2 (37) | 890 (12 us) |
| 3 | 1,478 | 2.71 ms | 38% | 40% | 21% | 0% | write 2,434 (46), free 335 (8), free_many 65 (39), read 17 (246), commit 2 (36) | 958 (12 us) |
| 4 | 1,450 | 2.76 ms | 38% | 40% | 21% | 0% | write 2,360 (48), free 350 (8), free_many 210 (21), read 18 (289), commit 2 (40) | 930 (13 us) |
| 5 | 1,436 | 2.79 ms | 37% | 41% | 22% | 1% | write 2,361 (56), free 355 (8), free_many 225 (22), read 18 (280), commit 2 (52) | 921 (13 us) |

## fxfs-guest-pgbench-dax

Load (pgbench -i): 19.82 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 14.55 s, vacuum 0.35 s, primary keys 4.91 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,394 | 2.87 ms | 36% | 39% | 24% | 1% | write 2,183 (66), free 277 (8), read 16 (277), free_many 12 (106), commit 2 (34) | 885 (9 us) |
| 2 | 1,364 | 2.93 ms | 37% | 41% | 22% | 1% | write 2,313 (55), free 331 (8), free_many 198 (22), read 16 (308), commit 2 (50) | 884 (12 us) |
| 3 | 1,450 | 2.76 ms | 38% | 40% | 21% | 0% | write 2,394 (47), free 315 (8), free_many 71 (36), read 17 (326), commit 2 (39) | 937 (13 us) |
| 4 | 1,428 | 2.80 ms | 38% | 41% | 20% | 0% | write 2,341 (46), free 355 (8), free_many 198 (20), read 17 (389), commit 2 (35) | 926 (13 us) |
| 5 | 1,407 | 2.84 ms | 37% | 40% | 22% | 1% | write 2,314 (58), free 344 (8), free_many 231 (22), read 18 (300), commit 2 (77) | 906 (13 us) |

## fxfs-guest-pgbench-dax-forked

Load (pgbench -i): 19.61 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.86 s, vacuum 0.47 s, primary keys 5.27 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,175 | 3.40 ms | 34% | 43% | 22% | 1% | – | – |
| 2 | 1,282 | 3.12 ms | 35% | 37% | 28% | 1% | write 1,768 (66), private 364 (51), map 293 (90), free 288 (9), read 16 (262), free_many 11 (165), commit 2 (852), unmap 0 (44), fault 0 (362) | 838 (9 us) |
| 3 | 1,234 | 3.24 ms | 33% | 34% | 32% | 1% | write 1,630 (78), map 583 (92), free 281 (24), free_many 191 (62), private 154 (436), fault 41 (79), read 16 (302), commit 2 (47), unmap 0 (42) | 777 (11 us) |
| 4 | 1,420 | 2.82 ms | 37% | 36% | 26% | 1% | write 1,872 (69), free 341 (13), free_many 219 (33), map 140 (91), private 83 (314), read 17 (279), fault 4 (26), commit 2 (52), unmap 0 (110) | 906 (12 us) |
| 5 | 1,267 | 3.16 ms | 33% | 33% | 33% | 1% | write 1,663 (92), map 472 (98), free 305 (23), free_many 193 (64), private 47 (1,340), read 15 (314), fault 11 (104), commit 2 (68), unmap 0 (24) | 793 (12 us) |

## fxfs-guest-pgbench-forked

Load (pgbench -i): 19.19 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.88 s, vacuum 0.57 s, primary keys 4.72 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,189 | 3.36 ms | 32% | 49% | 18% | 1% | – | – |
| 2 | 1,330 | 3.01 ms | 35% | 40% | 24% | 1% | write 2,258 (52), free 276 (8), free_many 24 (75), read 18 (272), commit 2 (39) | 852 (10 us) |
| 3 | 1,432 | 2.79 ms | 38% | 41% | 20% | 0% | write 2,348 (46), free 333 (8), free_many 246 (19), read 17 (263), commit 2 (58) | 924 (13 us) |
| 4 | 1,425 | 2.81 ms | 38% | 41% | 21% | 0% | write 2,339 (48), free 342 (8), free_many 218 (20), read 17 (304), commit 2 (45) | 914 (14 us) |
| 5 | 1,417 | 2.82 ms | 38% | 41% | 21% | 1% | write 2,335 (55), free 344 (8), free_many 222 (22), read 17 (296), commit 2 (54) | 910 (14 us) |

## pgbench-ext4-blk-unsafe

Load (pgbench -i): 14.25 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 10.15 s, vacuum 0.35 s, primary keys 3.74 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 2,591 | 1.54 ms | 46% | 44% | 9% | 0% | – | – |
| 2 | 2,725 | 1.47 ms | 47% | 45% | 8% | 0% | – | – |
| 3 | 2,782 | 1.44 ms | 47% | 44% | 9% | 0% | – | – |
| 4 | 2,782 | 1.44 ms | 50% | 43% | 7% | 0% | – | – |
| 5 | 2,725 | 1.47 ms | 48% | 45% | 7% | 0% | – | – |

## pgbench-ext4-blk-writeback

Load (pgbench -i): 19.31 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 13.87 s, vacuum 0.36 s, primary keys 5.06 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,102 | 3.63 ms | 24% | 22% | 54% | 0% | – | – |
| 2 | 1,166 | 3.43 ms | 24% | 20% | 56% | 0% | – | – |
| 3 | 1,122 | 3.56 ms | 23% | 21% | 56% | 0% | – | – |
| 4 | 1,112 | 3.60 ms | 23% | 21% | 56% | 0% | – | – |
| 5 | 1,136 | 3.52 ms | 24% | 21% | 55% | 0% | – | – |

## pgbench-ext4-dax-pmem

Load (pgbench -i): 23.57 s: drop tables 0.01 s, create tables 0.01 s, client-side generate 15.55 s, vacuum 1.46 s, primary keys 6.53 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,175 | 3.40 ms | 30% | 26% | 44% | 0% | – | – |
| 2 | 1,154 | 3.46 ms | 30% | 27% | 44% | 0% | – | – |
| 3 | 1,196 | 3.35 ms | 30% | 27% | 44% | 0% | – | – |
| 4 | 1,202 | 3.33 ms | 29% | 27% | 43% | 0% | – | – |
| 5 | 1,211 | 3.30 ms | 30% | 27% | 43% | 0% | – | – |

## fxfs-guest-pgbench-dax-density

Load (pgbench -i): 20.55 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 14.37 s, vacuum 0.35 s, primary keys 5.83 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,199 | 3.34 ms | 34% | 43% | 23% | 1% | – | – |
| 2 | 1,302 | 3.07 ms | 34% | 37% | 28% | 1% | write 1,807 (72), private 335 (48), free 296 (9), map 296 (83), read 16 (253), free_many 12 (147), commit 2 (100), unmap 0 (58), fault 0 (220) | 856 (9 us) |
| 3 | 1,197 | 3.34 ms | 35% | 39% | 25% | 1% | – | – |
| 4 | 1,220 | 3.28 ms | 34% | 36% | 29% | 1% | write 1,807 (72), private 335 (48), free 296 (9), map 296 (83), read 16 (253), free_many 12 (147), commit 2 (100), unmap 0 (58), fault 0 (220) | 856 (9 us) |
| 5 | 1,267 | 3.16 ms | 36% | 40% | 24% | 1% | – | – |
| 6 | 1,247 | 3.21 ms | 33% | 36% | 30% | 1% | write 1,745 (71), private 338 (47), map 296 (83), free 283 (9), read 15 (251), free_many 11 (144), commit 2 (102), fault 1 (72), unmap 0 (57) | 827 (9 us) |

## fxfs-guest-pgbench-density

Load (pgbench -i): 19.41 s: drop tables 0.00 s, create tables 0.01 s, client-side generate 13.82 s, vacuum 0.49 s, primary keys 5.09 s.

| Run | tps | Latency | Guest user | System | Idle | Steal | extentd requests/s (mean us) | Commits/s (mean wait) |
|---|---|---|---|---|---|---|---|---|
| 1 | 1,172 | 3.41 ms | 33% | 46% | 20% | 1% | – | – |
| 2 | 1,168 | 3.42 ms | 33% | 42% | 23% | 1% | write 2,110 (48), read 1,275 (128), free 258 (8), free_many 19 (100), commit 2 (168) | 799 (10 us) |
| 3 | 1,141 | 3.51 ms | 32% | 47% | 20% | 1% | – | – |
| 4 | 1,191 | 3.36 ms | 34% | 42% | 23% | 1% | write 2,082 (48), read 1,259 (100), free 253 (8), free_many 18 (100), commit 2 (170) | 788 (10 us) |
| 5 | 1,177 | 3.40 ms | 33% | 46% | 20% | 1% | – | – |
| 6 | 1,143 | 3.50 ms | 33% | 42% | 24% | 1% | write 2,110 (48), read 1,275 (128), free 258 (8), free_many 19 (100), commit 2 (168) | 799 (10 us) |
