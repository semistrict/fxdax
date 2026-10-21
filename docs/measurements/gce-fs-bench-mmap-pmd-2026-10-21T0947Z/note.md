TASK-26.18.8: Fxfs's DAX read faults map 2 MiB at once (huge_fault, read-only), mappings 2 MiB-aligned (thp_get_unmapped_area); against gce-fs-bench-mmap-base (4 KiB faults: mmap-seq-read 2,000 MiB/s, 261 faults/MiB; ext4 9,481, 5.6).

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: io. 
