TASK-25.3 step D: FUSE_SETUPMAPPING's ZX_VMAR_OP_MAP_RANGE hands the host MMU up to 512 pages at a time rather than 16, for longer runs of physically consecutive pages, each one host mapping and one KVM pre-fault.

Iterations: 3. Cases: bench-fs-fxdax-dax. Workloads: io.
