Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.01 (3.00–3.29) | 3.02 (2.95–3.10) | 1.00× |
| pgbench-tps | tps | 1,713 (1,621–1,730) | 1,475 (1,462–1,490) | 1.16× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.02 median (-4.07–-3.33)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.18 median (-3.92–-1.22)

Every workload is within 2× of ext4.
