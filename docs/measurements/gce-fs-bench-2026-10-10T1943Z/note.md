TASK-25.2 diagnostic: fxdax's server timing each FUSE opcode (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: metadata.

Finding: inside fxdax, a request took 15-35 µs on average (LOOKUP 29, GETATTR 14, CREATE 25,
UNLINK 28, WRITE 34). The cold stats are one LOOKUP each, at ~208 µs a stat: ~180 µs of it is
outside fxdax, in the transport and the guest, against ~80 µs for passthrough's whole request.
Creating a file is 6 requests: GETATTR of the parent (its attributes invalidated by each create),
LOOKUP, CREATE, WRITE, FLUSH and RELEASE.
