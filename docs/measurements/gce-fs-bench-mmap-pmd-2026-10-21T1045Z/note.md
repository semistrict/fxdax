TASK-26.18.8: 2 MiB read faults, and a file's whole extents mapped in its chunks' own window space (not at their regions), so that every chunk of a large file is one aligned run; against gce-fs-bench-mmap-base.

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: io. 
