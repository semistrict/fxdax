Profile of fxdax without DAX on the metadata workloads, for why a cold stat costs 4 times passthrough's.

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: metadata. Profiled with perf.

The profile sampled the whole host, which spent the run mostly rebuilding the tests: nothing of the
VMM's threads shows. Profiles are now of each case's VMM process.
