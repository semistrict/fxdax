TASK-92.28 targeted validation: appends already written through DAX use txn_update_size during inode writeback, without zeroing their extents.

Iterations: 1. Cases: fxfs-guest-checks-dax bench-fs-fxfs-guest-dax. Workloads: sqlite. 
