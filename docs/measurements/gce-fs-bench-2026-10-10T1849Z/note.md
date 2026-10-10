Profile of fxdax's VMM without DAX on the metadata workloads, for why a cold stat costs 4 times passthrough's.

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: metadata. Profiled with perf.

perf, attached to the VMM's process by its id, followed only the threads it had then, and took one
sample. The VMM now runs under perf from its start.
