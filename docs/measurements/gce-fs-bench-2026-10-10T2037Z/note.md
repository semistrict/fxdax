TASK-25.4 bisect: 2026-10-10T2035Z's write path with store faults dirtying one page, not runs of 16 (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io.

Failed too: mapping pages ahead of the guest's stores, not dirtying runs, brings on the EFAULT.
Diagnosis: the second stage replaces a page's host memory (mremap) and only then registers it
with userfaultfd; a vCPU's store between the two finds a read-only mapping KVM cannot fault, and
KVM_RUN fails with EFAULT. Mapping ahead makes the race likely; it was there before, for the
pages a fault maps after its own and for other vCPUs.
