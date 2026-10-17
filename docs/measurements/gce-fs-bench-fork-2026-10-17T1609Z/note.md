TASK-30: a VM booted on a fork of a base image (a copy of /usr and a pgbench database), served by the extent daemon that has the base's data in memory (warm) or by one of its own (cold): what the fork pays to start and run on the base's data, and where (extentd's counters).

Iterations: 3. Cases: bench-fork-fxfs-guest-dax-warm bench-fork-fxfs-guest-dax-cold. Workloads: all. 
