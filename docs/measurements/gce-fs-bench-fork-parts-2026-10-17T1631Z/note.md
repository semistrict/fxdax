TASK-38: page-ins of sealed extents read a whole part (1 MiB) at once rather than 128 KiB; compare with gce-fs-bench-fork-2026-10-17T1609Z.

Iterations: 3. Cases: bench-fork-fxfs-guest-dax-warm bench-fork-fxfs-guest-dax-cold. Workloads: all. 
