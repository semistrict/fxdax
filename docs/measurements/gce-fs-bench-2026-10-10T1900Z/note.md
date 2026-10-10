Profile of fxdax's VMM without DAX on the metadata workloads, for why a cold stat costs 4 times passthrough's.

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: metadata. Profiled with perf.

Finding: the VMM is mostly idle (7K samples in the run): the metadata workloads wait on latency.
What CPU time there is goes to the vCPUs' halts and exits, and to fuchsia-async's tokio workers
waking one another (contended mutexes, event listeners, spin locks): a request crosses from
libkrun's fs worker to fxdax's executor, to tokio workers for the in-process fuchsia.io
connection, and back.
