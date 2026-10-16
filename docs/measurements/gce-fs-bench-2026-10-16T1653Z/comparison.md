Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.10 (3.02–3.21) | 3.10 (3.09–3.24) | 1.00× |
| pgbench-tps | tps | 1,637 (1,617–1,645) | 1,220 (1,201–1,227) | 1.34× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -5.83 median (-6.00–-4.34)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -2.28 median (-3.62–-2.01)

Every workload is within 2× of ext4.
