TASK-25.5: libkrun's init sets the guest's clock from KVM's PTP clock, the host's, at boot.

Iterations: 1. Cases: bench-fs-ext4 bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata.

Finding: files on virtio-fs are stamped within 0.2 ms of the guest's clock (0.13 ms on fxdax,
-0.19 ms through passthrough), from 170 ms. ext4's metadata rates here (one iteration of the
metadata workloads alone, after boot) are well below the baseline's, which ran them after the I/O
workloads: not comparable with it.
