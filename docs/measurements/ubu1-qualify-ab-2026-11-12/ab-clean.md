| Workload | Unit | pin | main | main vs pin | Ranges overlap |
|---|---|---|---|---|---|
| soak.uploaded_after_answering | s | 50.05 (25.12–74.98) | 21.73 (20.93–22.53) | 0.43× | no |
| soak.guests.written | MiB | 1.536e+04 (1.536e+04–1.536e+04) | 1.536e+04 (1.536e+04–1.536e+04) | 1.00× | yes |
| soak.guests.longest_fsync | ms | 2.52e+05 (2.513e+05–2.527e+05) | 2.511e+05 (2.51e+05–2.513e+05) | 1.00× | no |
| soak.daemon.disk_written | GiB | 30.25 (30.24–30.25) | 15.2 (15.2–15.21) | 0.50× | no |
| soak.daemon.peak_rss | MiB | 4,359 (4,334–4,384) | 1,671 (1,640–1,703) | 0.38× | no |
| soak.daemon.cpu | s | 249.3 (247.2–251.3) | 385.6 (376.7–394.5) | 1.55× | no |
| budgets.sandboxes.max_anon | MiB | - | 985.5 (966.8–1,004) | - | - |
| budgets.sandboxes.io_per_guest_byte | x | - | 1.198 (1.192–1.203) | - | - |
| budgets.soak.max_anon | MiB | - | 1,589 (1,588–1,589) | - | - |
| budgets.soak.io_per_guest_byte | x | - | 1.007 (1.007–1.008) | - | - |
| soak.daemon.max_anon | MiB | - | 1,588 (1,587–1,589) | - | - |
| sandboxes.vms | s | - | 798 (792.8–803.2) | - | - |
| sandboxes.vm.builder | s | - | 777.3 (770.1–784.4) | - | - |
| sandboxes.vm.git | s | - | 114.8 (107.6–122) | - | - |
| sandboxes.vm.pgbench | s | - | 610.2 (605.1–615.4) | - | - |
| sandboxes.vm.writer | s | - | 84.99 (81.76–88.22) | - | - |
| sandboxes.command.cat | ms | - | 63 (57–69) | - | - |
| sandboxes.command.echo | ms | - | 68.5 (60–77) | - | - |
| sandboxes.command.uname | ms | - | 123 (62–184) | - | - |
| sandboxes.framings.slow | count | - | 1,400 (1,398–1,402) | - | - |
| sandboxes.framings.slowest | ms | - | 3,172 (1,640–4,704) | - | - |
| sandboxes.framings.sum | s | - | 277.8 (277.7–277.9) | - | - |
| sandboxes.daemons.disk_written | GiB | - | 90.48 (90.43–90.53) | - | - |
| sandboxes.daemons.peak_rss | MiB | - | 3,110 (3,091–3,128) | - | - |
| sandboxes.daemons.cpu | s | - | 1,121 (1,116–1,125) | - | - |
| sandboxes.guests.written | GiB | - | 75.55 (75.17–75.92) | - | - |
| sandboxes.stores.per_guest_byte | x | - | 1.195 (1.19–1.2) | - | - |
| sandboxes.daemons.disk_per_guest_byte | x | - | 1.198 (1.192–1.203) | - | - |

Moved, their ranges apart: soak.uploaded_after_answering, soak.guests.longest_fsync, soak.daemon.disk_written, soak.daemon.peak_rss, soak.daemon.cpu.

| Failed | pin | main |
|---|---|---|
| budgets.unreported | 2 of 2 | 0 of 2 |
| framings | 0 of 2 | 2 of 2 |
| memory | 2 of 2 | 0 of 2 |
| soak.failures | 2 of 2 | 0 of 2 |
| soak.memory | 2 of 2 | 0 of 2 |
| vm | 0 of 2 | 1 of 2 |

- pin-r2: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says nothing its budgets weigh
- pin-r2: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-b-stats.txt: says nothing its budgets weigh
- pin-r2: soak/fxfs-guest-bucket-soak/fxfs-guest-bucket-soak/extentd-stats.txt: says nothing its budgets weigh
- pin-r2: memory: its stats have no process: line: it weighs no memory against a budget
- pin-r2: failures: the bucket full, and no failure said
- pin-r2: failures: the bucket out of service, and no failure said
- pin-r2: memory: the run's memory ran out: 15379 MiB held past reclaim of its 16384 MiB, its daemons holding host-a 4326 MiB and host-b 4839 MiB
- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says nothing its budgets weigh
- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-b-stats.txt: says nothing its budgets weigh
- pin-r1: soak/fxfs-guest-bucket-soak/fxfs-guest-bucket-soak/extentd-stats.txt: says nothing its budgets weigh
- pin-r1: memory: its stats have no process: line: it weighs no memory against a budget
- pin-r1: failures: the bucket full, and no failure said
- pin-r1: failures: the bucket out of service, and no failure said
- pin-r1: memory: the run's memory ran out: 15420 MiB held past reclaim of its 16384 MiB, its daemons holding host-a 4326 MiB and host-b 5569 MiB
- main-r2: framings: host-a framed 10 segments slower than 1000 ms, the slowest 1549.6 ms
- main-r2: framings: host-b framed 12 segments slower than 1000 ms, the slowest 4704.1 ms
- main-r1: vm: sandbox-6-git exited exit status: 0, saying ""
- main-r1: vm: sandbox-2-git exited exit status: 0, saying ""
- main-r1: framings: host-a framed 4 segments slower than 1000 ms, the slowest 1220.8 ms
- main-r1: framings: host-b framed 10 segments slower than 1000 ms, the slowest 1640.1 ms
