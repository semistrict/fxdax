TASK-25.4 diagnostic: step B failed with a vCPU's KVM_RUN returning EFAULT during sequential writes; logging writes the window refuses (not committed).

Iterations: 1. Cases: bench-fs-fxdax-dax. Workloads: io.

Failed as 2026-10-10T2029Z did. The window refused no write (nothing logged), so the EFAULT is not
the read-only resolution of a refused write.
