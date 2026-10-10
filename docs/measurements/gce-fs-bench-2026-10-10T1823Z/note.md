Baseline: the first run with the workloads as they now are (256 MiB data, Alpine root filesystem,
Lua build, 3 iterations) and libkrun's virtio-fs passthrough as a control
([write-up](gce-fs-bench-2026-10-10T1823Z.md)). Warm sequential reads take ~40 ms, which fio's
millisecond runtimes quantize.
