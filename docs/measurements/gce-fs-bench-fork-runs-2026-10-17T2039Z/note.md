TASK-38: page-ins commit their buffers' pages at once, so that a part paged in is one run of physical memory, which a window maps with one host mapping.

Iterations: 3. Cases: bench-fork-fxfs-guest-dax-warm bench-fork-fxfs-guest-dax-cold bench-fork-fxfs-guest-dax-profiled. Workloads: all. 
