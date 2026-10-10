TASK-25.7 experiment: KVM halt polling off on the host (halt_poll_ns 0, from 200000), fxdax against passthrough on the metadata workloads, with each device's time per request (not committed).

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata.

Finding: without halt polling, passthrough's cold stats fall from ~9,900 to 7,065 a second, and
fxdax's stay at ~5,100. Passthrough's replies come while the waiting vCPU still polls; fxdax's
come too late, and its vCPU sleeps and is woken each time. Restored to 200000 after the run.
