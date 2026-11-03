# pgbench-dax: a write(2)'s move committed after its bytes, against main (TASK-153.5)

2026-11-03, one host (GCE n2-standard-4, us-west1-a, fxdax-test-t1535-ab), `fxfs-guest-pgbench-dax`
(scale 50, five 60 s runs of `-c 4 -j 2` a round), interleaved a, b, a, b..., five rounds a side.

- **a**: main, 457d500b0.
- **b**: the prototype: 4918a60bd in round 1, e1d89f2d8 in rounds 2 to 5 (e1d89f2d8 adds the slice
  remap of c9013caec and test changes; nothing on pgbench's path).

## tps

| Round | a (main) | b (commit after bytes) | b / a |
|---|---|---|---|
| 1 | 846.7 794.9 809.7 812.9 788.8 (810.6) | 884.9 808.5 827.2 821.4 810.1 (830.4) | 1.024 |
| 2 | 928.6 843.5 851.2 863.6 874.5 (872.3) | 905.3 829.1 827.9 812.6 812.7 (837.5) | 0.960 |
| 3 | 903.8 816.2 822.3 806.4 794.5 (828.7) | 904.4 832.9 847.0 838.5 855.4 (855.6) | 1.033 |
| 4 | 891.2 816.9 822.1 813.5 748.7 (818.5) | 868.9 795.1 742.2 770.5 809.1 (797.2) | 0.974 |
| 5 | 900.5 825.3 821.2 825.1 835.6 (841.5) | 877.9 760.2 801.2 810.0 805.4 (810.9) | 0.964 |
| All 25 | mean 834.3, median 822.3, sd 41.0 | mean 826.3, median 821.4, sd 40.8 | 0.990 |

The rounds' ratios alternate around 1; the difference of the means, 8 tps, is under one standard
error of it (about 11.6). Equal.

## Where a write(2)'s time went (dax stats, all rounds, per write)

| | a | b |
|---|---|---|
| writes | 4,339,324 | 4,372,611 |
| through a region | 97.2% | 97.5% |
| begin (iomap_begin) | 295.8 us | 274.8 us |
| of it, in Fxfs | 57.6 us | 33.8 us |
| end (iomap_end, b's commit) | 0.3 us | 25.7 us |
| copy | 15.5 us | 16.5 us |
| inode lock wait | 82.9 us | 84.1 us |
| fault-in | 2.1 us | 0.3 us |
| moves dropped (a seal between move and gate) | 0 | 31,966 (0.73%) |

Fxfs's share is the same, split in two calls (57.6 against 33.8 + about 25). What b no longer pays
(the bytes in hand first, a second move after a seal) is small next to the round trips.
