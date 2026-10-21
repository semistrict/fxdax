TASK-26.18.8 baseline: mmap-seq-read (fio, one pass through a new mapping) and sqlite-mmap-scan (SQLite with mmap_size, a 256 MiB table scanned by a new process), with the guest's page faults per MiB; before 2 MiB faults.

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: io. 
