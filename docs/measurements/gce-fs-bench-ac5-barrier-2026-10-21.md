# TASK-42.7 AC5 after the fill barrier: a writer alone, unchanged

The fill barrier (TASK-42.10, a17eb73) gave a follower's inodes operation tables of their own and put
its other paths behind the followers' static key: a writer's mounts take none of it. Measured: a writer
alone (bench-fs-fxfs-guest-dax: io, metadata and pgbench, 3 iterations) on the commit before it
(aad4dec) and on it (a17eb73), each with its own guest kernel, alternating, twice each, on one GCE host
(fxdax-bench-follow1, c3-standard-8, us-central1-a), 2026-10-21. Every workload's ranges overlap:
within noise. With the earlier noise check (gce-fs-bench-ac5-*-2026-10-19, the commit before the
follower code against the followers', before the barrier), a writer alone runs as it did before the
follower code.

Runs: gce-fs-bench-ac5-barrier-aad4dec-2026-10-21T0945Z, -T1002Z; gce-fs-bench-ac5-barrier-a17eb73-2026-10-21T0956Z, -T1008Z.
Table: `scripts/bench-ab.py aad4dec <aad4dec runs> -- a17eb73 <a17eb73 runs>` (6 values a side).

| Workload | Unit | aad4dec | a17eb73 | a17eb73 vs aad4dec | Ranges overlap |
|---|---|---|---|---|---|
| seq-write | MiB/s | 70.05 (65.8–70.32) | 65.8 (65.57–70.31) | 1.06× | yes |
| rand-read-cold | IOPS | 1.518e+05 (1.085e+05–1.883e+05) | 1.554e+05 (1.1e+05–1.95e+05) | 0.98× | yes |
| seq-read-cold | MiB/s | 1,060 (719.1–1,399) | 1,081 (709.1–1,463) | 0.98× | yes |
| seq-read-warm | MiB/s | 1.102e+04 (1.078e+04–1.136e+04) | 1.109e+04 (1.078e+04–1.125e+04) | 0.99× | yes |
| rand-read-warm | IOPS | 7.825e+05 (7.708e+05–8.032e+05) | 8.046e+05 (7.763e+05–8.077e+05) | 0.97× | yes |
| rand-write | IOPS | 1.182e+04 (1.181e+04–1.442e+04) | 1.182e+04 (1.181e+04–1.322e+04) | 1.00× | yes |
| mmap-rand-read | IOPS | 1.108e+06 (1.093e+06–1.129e+06) | 1.125e+06 (1.1e+06–1.142e+06) | 0.98× | yes |
| mmap-rand-write | IOPS | 1.182e+04 (1.181e+04–1.442e+04) | 1.182e+04 (1.181e+04–1.442e+04) | 1.00× | yes |
| fsync-append | IOPS | 402.2 (395–655.3) | 400 (387.8–633.6) | 1.01× | yes |
| create | ops/s | 2.586e+04 (2.445e+04–2.786e+04) | 2.618e+04 (2.525e+04–2.653e+04) | 0.99× | yes |
| stat-cold | ops/s | 2.816e+05 (1.831e+05–2.939e+05) | 2.881e+05 (1.754e+05–2.971e+05) | 0.98× | yes |
| stat-warm | ops/s | 1.283e+06 (9.257e+05–1.344e+06) | 1.233e+06 (8.669e+05–1.428e+06) | 1.04× | yes |
| lookup-missing | ops/s | 3.887e+05 (2.862e+05–4.91e+05) | 3.577e+05 (2.934e+05–4.99e+05) | 1.09× | yes |
| readdir | ops/s | 1.994e+06 (7.448e+05–2.206e+06) | 2.055e+06 (1.111e+06–2.086e+06) | 0.97× | yes |
| read-small-cold | ops/s | 1.114e+05 (1.06e+05–1.196e+05) | 1.144e+05 (1.105e+05–1.165e+05) | 0.97× | yes |
| chmod | ops/s | 1.182e+06 (8.617e+05–1.372e+06) | 1.272e+06 (1.119e+06–1.348e+06) | 0.93× | yes |
| setxattr | ops/s | 5.45e+05 (2.335e+05–6.183e+05) | 6.213e+05 (4.355e+05–6.539e+05) | 0.88× | yes |
| link | ops/s | 9.024e+04 (8.32e+04–9.757e+04) | 9.381e+04 (8.969e+04–9.849e+04) | 0.96× | yes |
| rename | ops/s | 7.26e+04 (6.292e+04–8.123e+04) | 7.341e+04 (6.896e+04–8.255e+04) | 0.99× | yes |
| rename-across | ops/s | 6.444e+04 (5.76e+04–7.204e+04) | 6.344e+04 (5.827e+04–7.524e+04) | 1.02× | yes |
| unlink | ops/s | 1.836e+05 (1.665e+05–2.874e+05) | 1.822e+05 (1.545e+05–2.467e+05) | 1.01× | yes |
| create-fsync | ops/s | 1,518 (1,191–1,939) | 1,483 (1,055–1,733) | 1.02× | yes |
| mkdir-tree | ops/s | 2.742e+04 (2.411e+04–2.84e+04) | 2.533e+04 (1.709e+04–2.786e+04) | 1.08× | yes |
| find-cold | ops/s | 2.222e+05 (2.097e+05–2.358e+05) | 2.202e+05 (1.812e+05–2.258e+05) | 1.01× | yes |
| rm-tree | ops/s | 5.956e+04 (5.682e+04–6.766e+04) | 6.305e+04 (5.925e+04–6.534e+04) | 0.94× | yes |
| large-dir-create | ops/s | 8.443e+04 (6.018e+04–8.944e+04) | 8.25e+04 (5.837e+04–8.869e+04) | 1.02× | yes |
| large-dir-stat-cold | ops/s | 2.521e+05 (1.598e+05–2.619e+05) | 2.611e+05 (2.498e+05–2.836e+05) | 0.97× | yes |
| large-dir-unlink | ops/s | 2.134e+05 (1.705e+05–2.242e+05) | 2.128e+05 (1.708e+05–2.381e+05) | 1.00× | yes |
| create-parallel | ops/s | 1.163e+05 (1.092e+05–1.212e+05) | 1.108e+05 (9.703e+04–1.143e+05) | 1.05× | yes |
| stat-parallel | ops/s | 5.383e+05 (4.032e+05–6.27e+05) | 5.122e+05 (4.6e+05–5.891e+05) | 1.05× | yes |
| unlink-parallel | ops/s | 1.736e+05 (1.584e+05–2.044e+05) | 1.747e+05 (1.727e+05–1.894e+05) | 0.99× | yes |
| create-shared-dir | ops/s | 8.27e+04 (8.202e+04–8.858e+04) | 8.061e+04 (7.738e+04–8.424e+04) | 1.03× | yes |
| unlink-shared-dir | ops/s | 1.519e+05 (1.426e+05–1.867e+05) | 1.631e+05 (1.453e+05–1.826e+05) | 0.93× | yes |
| pgbench-init | s | 3.821 (3.629–3.922) | 3.881 (3.772–4.006) | 1.02× | yes |
| pgbench-tps | tps | 1,592 (1,587–1,609) | 1,591 (1,587–1,614) | 1.00× | yes |
