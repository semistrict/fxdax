| Workload | Unit | pin | main | main vs pin | Ranges overlap |
|---|---|---|---|---|---|
| soak.uploaded_after_answering | s | 74.98 (74.98–74.98) | 20.93 (20.93–20.93) | 0.28× | no |
| soak.guests.written | MiB | 1.536e+04 (1.536e+04–1.536e+04) | 1.536e+04 (1.536e+04–1.536e+04) | 1.00× | yes |
| soak.guests.longest_fsync | ms | 2.527e+05 (2.527e+05–2.527e+05) | 2.51e+05 (2.51e+05–2.51e+05) | 0.99× | no |
| soak.daemon.disk_written | GiB | 30.24 (30.24–30.24) | 15.2 (15.2–15.2) | 0.50× | no |
| soak.daemon.peak_rss | MiB | 4,334 (4,334–4,334) | 1,703 (1,703–1,703) | 0.39× | no |
| soak.daemon.cpu | s | 251.3 (251.3–251.3) | 376.7 (376.7–376.7) | 1.50× | no |
| budgets.sandboxes.max_anon | MiB | - | 1,004 (1,004–1,004) | - | - |
| budgets.sandboxes.io_per_guest_byte | x | - | 1.203 (1.203–1.203) | - | - |
| budgets.soak.max_anon | MiB | - | 1,589 (1,589–1,589) | - | - |
| budgets.soak.io_per_guest_byte | x | - | 1.007 (1.007–1.007) | - | - |
| soak.daemon.max_anon | MiB | - | 1,589 (1,589–1,589) | - | - |
| sandboxes.vms | s | - | 803.2 (803.2–803.2) | - | - |
| sandboxes.vm.builder | s | - | 784.4 (784.4–784.4) | - | - |
| sandboxes.vm.git | s | - | 122 (122–122) | - | - |
| sandboxes.vm.pgbench | s | - | 605.1 (605.1–605.1) | - | - |
| sandboxes.vm.writer | s | - | 88.22 (88.22–88.22) | - | - |
| sandboxes.command.cat | ms | - | 57 (57–57) | - | - |
| sandboxes.command.echo | ms | - | 77 (77–77) | - | - |
| sandboxes.command.uname | ms | - | 184 (184–184) | - | - |
| sandboxes.framings.slow | count | - | 1,402 (1,402–1,402) | - | - |
| sandboxes.framings.slowest | ms | - | 1,640 (1,640–1,640) | - | - |
| sandboxes.framings.sum | s | - | 277.9 (277.9–277.9) | - | - |
| sandboxes.daemons.disk_written | GiB | - | 90.43 (90.43–90.43) | - | - |
| sandboxes.daemons.peak_rss | MiB | - | 3,128 (3,128–3,128) | - | - |
| sandboxes.daemons.cpu | s | - | 1,125 (1,125–1,125) | - | - |
| sandboxes.guests.written | GiB | - | 75.17 (75.17–75.17) | - | - |
| sandboxes.stores.per_guest_byte | x | - | 1.2 (1.2–1.2) | - | - |
| sandboxes.daemons.disk_per_guest_byte | x | - | 1.203 (1.203–1.203) | - | - |

Moved, their ranges apart: soak.uploaded_after_answering, soak.guests.longest_fsync, soak.daemon.disk_written, soak.daemon.peak_rss, soak.daemon.cpu.

| Failed | pin | main |
|---|---|---|
| budgets.unreported | 1 of 1 | 0 of 1 |
| framings | 0 of 1 | 1 of 1 |
| memory | 1 of 1 | 0 of 1 |
| soak.failures | 1 of 1 | 0 of 1 |
| soak.memory | 1 of 1 | 0 of 1 |
| vm | 0 of 1 | 1 of 1 |

- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says nothing its budgets weigh
- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-b-stats.txt: says nothing its budgets weigh
- pin-r1: soak/fxfs-guest-bucket-soak/fxfs-guest-bucket-soak/extentd-stats.txt: says nothing its budgets weigh
- pin-r1: memory: its stats have no process: line: it weighs no memory against a budget
- pin-r1: failures: the bucket full, and no failure said
- pin-r1: failures: the bucket out of service, and no failure said
- pin-r1: memory: the run's memory ran out: 15420 MiB held past reclaim of its 16384 MiB, its daemons holding host-a 4326 MiB and host-b 5569 MiB
- main-r1: vm: sandbox-6-git exited exit status: 0, saying ""
- main-r1: vm: sandbox-2-git exited exit status: 0, saying ""
- main-r1: framings: host-a framed 4 segments slower than 1000 ms, the slowest 1220.8 ms
- main-r1: framings: host-b framed 10 segments slower than 1000 ms, the slowest 1640.1 ms
