Candidate4: append reservation plus extent-result buffers sized to covered logical pages rather than 512 entries. Root stage experiment reverted. Same-host full SQLite comparison, Fxfs then ext4, correctness checks unchanged.

Iterations: 3. Cases: bench-fs-fxfs-guest-dax bench-fs-ext4-dax-pmem. Workloads: sqlite.

Outcome: build failed: test snapshot had a stats type error
