TASK-92.28/TASK-92.58: DAX appends into writable blocks defer size to inode writeback using txn_update_size, without a Fxfs transaction per write or zeroing their data; full guest suites and 3-iteration SQLite validation.

Iterations: 3. Cases: fxfs-guest-checks-dax fxfs-guest-checks fxfs-guest fxfs-guest-dax bench-fs-fxfs-guest-dax. Workloads: sqlite. 
