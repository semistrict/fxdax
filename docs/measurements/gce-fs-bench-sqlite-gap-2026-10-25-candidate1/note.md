First append-allocation candidate: sustained appends reserve up to 16 pages without advancing EOF; three unprofiled SQLite iterations on the baseline host. Includes all recovery, integrity, payload and snapshot checks.

Iterations: 3. Cases: bench-fs-fxfs-guest-dax. Workloads: sqlite.

Outcome: retained: sustained append reservation
