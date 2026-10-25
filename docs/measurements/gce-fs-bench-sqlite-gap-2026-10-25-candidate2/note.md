Candidate 2 experiment: append reservation plus bounded sequential overwrite COW preserving every lookahead byte, clone data and mapped-reader coherence. Same host, Fxfs then ext4, full unprofiled SQLite group with correctness checks.

Iterations: 3. Cases: bench-fs-fxfs-guest-dax bench-fs-ext4-dax-pmem. Workloads: sqlite.

Outcome: discarded: overwrite lookahead increased checkpoint cost
