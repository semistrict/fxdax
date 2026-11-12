| Workload | Unit | pin | main | main vs pin | Ranges overlap |
|---|---|---|---|---|---|
| soak.uploaded_after_answering | s | 42.9 (25.12–60.67) | 23.75 (22.53–24.98) | 0.55× | no |
| soak.guests.written | MiB | 1.536e+04 (1.536e+04–1.536e+04) | 1.536e+04 (1.536e+04–1.536e+04) | 1.00× | yes |
| soak.guests.longest_fsync | ms | 2.543e+05 (2.513e+05–2.572e+05) | 2.524e+05 (2.513e+05–2.536e+05) | 0.99× | yes |
| soak.daemon.disk_written | GiB | 30.26 (30.25–30.27) | 15.2 (15.2–15.21) | 0.50× | no |
| soak.daemon.peak_rss | MiB | 4,624 (4,384–4,864) | 1,576 (1,512–1,640) | 0.34× | no |
| soak.daemon.cpu | s | 240.8 (234.4–247.2) | 385.2 (375.9–394.5) | 1.60× | no |
| budgets.sandboxes.max_anon | MiB | - | 1,007 (966.8–1,047) | - | - |
| budgets.sandboxes.io_per_guest_byte | x | - | 1.198 (1.192–1.204) | - | - |
| budgets.soak.max_anon | MiB | - | 1,520 (1,452–1,588) | - | - |
| budgets.soak.io_per_guest_byte | x | - | 1.007 (1.007–1.008) | - | - |
| soak.daemon.max_anon | MiB | - | 1,519 (1,451–1,587) | - | - |
| sandboxes.vms | s | - | 866 (792.8–939.1) | - | - |
| sandboxes.vm.builder | s | - | 847.2 (770.1–924.2) | - | - |
| sandboxes.vm.git | s | - | 104.6 (101.6–107.6) | - | - |
| sandboxes.vm.pgbench | s | - | 678.4 (615.4–741.3) | - | - |
| sandboxes.vm.writer | s | - | 71.3 (60.84–81.76) | - | - |
| sandboxes.command.cat | ms | - | 69.5 (69–70) | - | - |
| sandboxes.command.echo | ms | - | 59.5 (59–60) | - | - |
| sandboxes.command.uname | ms | - | 56.5 (51–62) | - | - |
| sandboxes.framings.slow | count | - | 1,486 (1,398–1,574) | - | - |
| sandboxes.framings.slowest | ms | - | 5,328 (4,704–5,952) | - | - |
| sandboxes.framings.sum | s | - | 312.2 (277.7–346.7) | - | - |
| sandboxes.daemons.disk_written | GiB | - | 91.42 (90.53–92.3) | - | - |
| sandboxes.daemons.peak_rss | MiB | - | 3,115 (3,091–3,139) | - | - |
| sandboxes.daemons.cpu | s | - | 1,215 (1,116–1,313) | - | - |
| sandboxes.guests.written | GiB | - | 76.3 (75.92–76.69) | - | - |
| sandboxes.stores.per_guest_byte | x | - | 1.195 (1.19–1.201) | - | - |
| sandboxes.daemons.disk_per_guest_byte | x | - | 1.198 (1.192–1.204) | - | - |

Moved, their ranges apart: soak.uploaded_after_answering, soak.daemon.disk_written, soak.daemon.peak_rss, soak.daemon.cpu.

| Failed | pin | main |
|---|---|---|
| budgets.unreported | 2 of 2 | 0 of 2 |
| framings | 0 of 2 | 2 of 2 |
| memory | 2 of 2 | 0 of 2 |
| soak.failures | 2 of 2 | 0 of 2 |
| soak.memory | 2 of 2 | 0 of 2 |

- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says nothing its budgets weigh
- pin-r1: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-b-stats.txt: says nothing its budgets weigh
- pin-r1: soak/fxfs-guest-bucket-soak/fxfs-guest-bucket-soak/extentd-stats.txt: says nothing its budgets weigh
- pin-r1: memory: its stats have no process: line: it weighs no memory against a budget
- pin-r1: failures: the bucket full, and no failure said
- pin-r1: failures: the bucket out of service, and no failure said
- pin-r1: memory: the run's memory ran out: 15365 MiB held past reclaim of its 16384 MiB, its daemons holding host-a 4328 MiB and host-b 4652 MiB
- pin-r2: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says nothing its budgets weigh
- pin-r2: sandboxes/fxfs-guest-sandboxes/fxfs-guest-sandboxes/host-b-stats.txt: says nothing its budgets weigh
- pin-r2: soak/fxfs-guest-bucket-soak/fxfs-guest-bucket-soak/extentd-stats.txt: says nothing its budgets weigh
- pin-r2: memory: its stats have no process: line: it weighs no memory against a budget
- pin-r2: failures: the bucket full, and no failure said
- pin-r2: failures: the bucket out of service, and no failure said
- pin-r2: memory: the run's memory ran out: 15379 MiB held past reclaim of its 16384 MiB, its daemons holding host-a 4326 MiB and host-b 4839 MiB
- main-r1: framings: host-a framed 18 segments slower than 1000 ms, the slowest 4720.8 ms
- main-r1: framings: host-b framed 7 segments slower than 1000 ms, the slowest 5952.0 ms
- main-r2: framings: host-a framed 10 segments slower than 1000 ms, the slowest 1549.6 ms
- main-r2: framings: host-b framed 12 segments slower than 1000 ms, the slowest 4704.1 ms
