TASK-267: KVM exits per request, virtio transport mmio (Firecracker 87ddcbefa)

Iterations: 1. Cases: bench-fs-fxfs-guest bench-fs-fxfs-guest-dax. Workloads: fsync. perf: stat:kvm:kvm_exit,kvm:kvm_userspace_exit,kvm:kvm_mmio,kvm:kvm_msi_set_irq. 
