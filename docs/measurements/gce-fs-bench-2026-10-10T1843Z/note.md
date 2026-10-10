Profiling run for what to improve: fxdax with and without DAX and passthrough, the I/O and
metadata workloads, perf sampling the host. The profile's report shows only the idle loop
(the call-graph report), so only the measurements are of use.

Iterations: 1. Cases: bench-fs-fxdax-dax bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: io,metadata. Profiled with perf.
