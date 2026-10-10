TASK-25.5 diagnostic: whether the guest has the KVM PTP clock, and the host's time through it against the guest's (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax. Workloads: none.

Finding: the guest has KVM's PTP clock (/dev/ptp0, "KVM virtual PTP"), which reads the host's
clock in ~10 µs: the host's is 169.6 ms ahead of the guest's through it, as the mtime observation
has it (169.7 ms).
