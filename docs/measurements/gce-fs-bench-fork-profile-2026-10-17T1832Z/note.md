TASK-38: boot profiles: a fork records the base's units it reads in its first 60 s; forks opened after it read them into the daemon's cache as they boot (profiled), against warm and cold.

Iterations: 3. Cases: bench-fork-fxfs-guest-dax-warm bench-fork-fxfs-guest-dax-cold bench-fork-fxfs-guest-dax-profiled. Workloads: all. 
