TASK-25.2 step B: libkrun's fxdax device loop runs on fxdax's executor, waiting on the queues' eventfds there, and handles requests concurrently.

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-fxdax-dax. Workloads: io,metadata.
