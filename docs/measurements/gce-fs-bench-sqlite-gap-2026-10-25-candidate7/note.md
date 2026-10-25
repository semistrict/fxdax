Candidate7 batch: C6 adjacent unmap batching extended to same-permission protections with matching ACK flags; cached DAX reads retain lookup lock, skip redundant LRU movement and empty wakeups using wq_has_sleeper barrier. C4buffer sizing and C1append reservation retained. Temporary per-kind logs. Same host full SQLite and correctness checks.

Iterations: 3. Cases: bench-fs-fxfs-guest-dax bench-fs-ext4-dax-pmem. Workloads: sqlite.

Outcome: protection batching discarded: no protection batches observed; cached-read kernel retained
