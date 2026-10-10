TASK-25.7: KVM's exits, exits to the VMM, vCPU wakeups, MMIO accesses and interrupts counted each 100 ms on the metadata workloads, fxdax against passthrough, for where a request's time goes outside the device.

Iterations: 1. Cases: bench-fs-fxdax-nodax bench-fs-passthrough. Workloads: metadata. perf: stat:kvm:kvm_exit,kvm:kvm_userspace_exit,kvm:kvm_vcpu_wakeup,kvm:kvm_mmio,kvm:kvm_set_irq.

Finding (logs, a line of counts each 100 ms): per request, both backends cost about the same in
KVM events: ~2 exits to the VMM (virtio-mmio's interrupt handler reads the interrupt status and
acknowledges it), ~7 MMIO accesses (with the queue notification, an ioeventfd) and ~1 vCPU wakeup.
fxdax's slower cold stats are not more exits.
