TASK-25.7 diagnostic: how long a request takes in each device, from taking it off the queue to signalling its reply, averaged over 2,000 (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata.

Finding (REQDIAG lines): a request takes 17-22 µs in fxdax's device, from taking it off the
queue to signalling its reply, against 7-10 µs in passthrough's: ~12 µs of the ~90 µs fxdax's
cold stats take longer each.
