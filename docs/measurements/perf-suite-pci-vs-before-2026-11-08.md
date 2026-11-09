# The performance suite on virtio-PCI main against main before it (TASK-275)

TASK-267 (1e8f3ff37) put every fxdax VM's devices on virtio-PCI and TASK-273 (751ced7b5) removed
fxdax's MMIO paths. virtio-pci-vs-mmio-2026-11-09.md measured the two transports on one tree. Here
the whole performance suite ran on main as it is against main before the switch: the PCI change
together with its Firecracker series, its kernel config and the window's placement. Each pair of
sides ran on one host, back to back.

**Trees.**

- *Old*: main at 557b49a70, the parent of 1e8f3ff37. Every device on virtio-MMIO, Firecracker
  fxdax-v1.16.1-r1 (8e14cd916), its own scripts. One local change, which only prints: 1e8f3ff37's
  `fxdax: boot kernel_ns=K agent_ns=A` console line (guest_init.rs, kernel_log.rs, measure.rs).
  Without it the old side's boot rounds would report no boot time. Each `source.txt` lists it as
  "changed".
- *New*: main at 0f011561a. Every device on virtio-PCI, Firecracker fxdax-v1.16.1-r2 (68d597175),
  `CONFIG_VIRTIO_MMIO` off in the guest kernel. New-side runs after the first fs bench round list
  `fs-bench-history.md` as "changed": that round's driver had regenerated it. No source changed.
- *New, restores*: main at 56cdb1ecd (Firecracker fxdax-v1.16.1-r3). 0f011561a hangs after a PCI
  restore (TASK-277, below), so its restore times come from the commit with the fix.

Each side built its own release guest kernel on the host.

**Hosts.**

| Suite | Host | Machine |
|---|---|---|
| fs bench, boot and restore, Linux checkout | `fxdax-keep-c3-standard-8-us-east4-a-1` | c3-standard-8, Sapphire Rapids, as virtio-pci-vs-mmio |
| perf gate, fork bench cases | `fxdax-keep-n2-standard-4-us-east1-b-1` | n2-standard-4, Cascade Lake, nested, as dax-cow-gate and restructure-before-after |
| `scripts/fork-time-gce.sh` | `fxdax-keep-n2-standard-4-us-west1-a-1` | n2-standard-4, both trees in one run |

All three ran kernel 7.0.0-1011-gcp.

The fs bench's results are in `gce-fs-bench-pci-ab-{old,new}-{1,2}-2026-11-08/` and
`-lookup-2026-11-09/`, where scripts/bench-history.py reads them. Everything else is in
perf-suite-pci-vs-before-2026-11-08/, curated: the logs, reports and JSON, without disk images,
buckets, snapshot memory, per-second pgbench samples or the daemons' full logs.

## Summary

Nothing measured is slower on main past its runs' spread, after a rerun. These are faster:

- Cold reads and anything synced: ext4 on virtio-blk's random cold reads 2.04×, Fxfs's 1.19-1.29×,
  creates with fsync 1.20-1.32×.
- pgbench: 1.08-1.20× in every perf gate case.
- A Linux checkout: 4-19% faster, and its cold reads 14-46% faster.

Unchanged: boot, restore, forks, DAX's warm and mapped workloads, and metadata that stays in the
guest.

Three things broke or need work, each filed:

- TASK-277: a restore over PCI hung on 0f011561a. Fixed on main since (56cdb1ecd).
- TASK-289: the perf gate's bigmap-read now fails, at 3.92 under 4.0. Neither side got slower: reads
  without DAX gained more than DAX's did.
- TASK-290: stat-warm lands in its slow mode twice as often on PCI. It is a ~1.7 ms measurement, so
  this may be an artifact.

One build failure, TASK-297: the old side's first fork-bench run lost five cases to lua.org timing
out. Those cases were rerun.

## fs bench

`scripts/bench-gce.sh run`, every bench-fs case (`ext4-guest-kernel`: ext4 on virtio-blk;
`ext4-pmem`; `ext4-dax-pmem`; `fxfs-guest`; `fxfs-guest-dax`; `fxfs-guest-dax-followed`;
`fxfs-guest-dax-followed-idle`), every group, three iterations, two rounds in the order old, new,
old, new. The 287 case-and-workload pairs, pooled over both rounds (6 values a side), with medians,
ranges and whether the ranges overlap: [fs-bench-ab.md](perf-suite-pci-vs-before-2026-11-08/fs-bench-ab.md).
Twelve differ beyond their ranges, all faster on PCI. One was slower and did not hold up on a rerun.

| Case | Workload | Old | New | New over old |
|---|---|---|---|---|
| ext4-guest-kernel | rand-read-cold (IOPS) | 14,487 (13,149-15,907) | 29,547 (26,299-31,447) | 2.04× |
| ext4-guest-kernel | read-small-cold (ops/s) | 15,268 (11,554-15,662) | 25,958 (23,372-26,326) | 1.70× |
| ext4-guest-kernel | fsync-append (IOPS) | 389 (387-395) | 435 (416-442) | 1.12× |
| fxfs-guest | read-small-cold | 6,377 (6,209-6,676) | 8,244 (7,982-8,618) | 1.29× |
| fxfs-guest | create-fsync (ops/s) | 2,247 (1,996-2,423) | 2,824 (2,500-3,097) | 1.26× |
| fxfs-guest | rand-read-cold | 5,018 (4,735-5,172) | 5,969 (5,648-6,088) | 1.19× |
| fxfs-guest-dax | fsync-append | 2,685 (2,409-2,768) | 3,284 (2,981-3,612) | 1.22× |
| fxfs-guest-dax | rand-write (IOPS) | 4,924 (4,872-5,177) | 5,996 (5,822-6,413) | 1.22× |
| fxfs-guest-dax | create-fsync | 2,348 (2,270-2,444) | 2,821 (2,708-3,032) | 1.20× |
| fxfs-guest-dax-followed-idle | create-fsync | 1,854 (1,760-1,919) | 2,448 (2,102-2,648) | 1.32× |
| fxfs-guest-dax-followed-idle | fsync-append | 2,058 (1,962-2,238) | 2,716 (2,606-3,039) | 1.32× |
| fxfs-guest-dax-followed-idle | rand-write | 5,024 (4,437-5,260) | 5,970 (5,881-6,438) | 1.19× |
| ext4-guest-kernel | lookup-missing (ops/s) | 456,116 (452,295-458,015) | 437,056 (381,147-447,612) | 0.96× |

The one slower pair, ext4's lookup-missing, came out 0.97× with overlapping ranges on a rerun of
ext4-guest-kernel's metadata group, five iterations each side:
`gce-fs-bench-pci-ab-*-lookup-2026-11-09`. It is noise: the old side shows the same dips to
300-390k in other cases.

**stat-warm (TASK-290).** Both sides are bimodal, about 0.7-0.85M or 1.1-1.4M stats/s per run.
Main lands in the slow mode in 20 of 47 runs, against 10 of 47 for the old tree. On ext4-guest-kernel,
with the rerun, that is 6 of 11 against 0 of 11. The workload is one pass of `fs::metadata` over
the metadata group's files, about 1.7 ms, so one interrupt halves it. A late device completion over
PCI may be what lands in it, or a real per-stat cost. Two cases, the followed ones' cold reads, are
also bimodal on both sides (11k or 93k IOPS a run), and their medians differ by which mode the runs
hit.

## Boot and restore

`scripts/test-gce.sh run`, `extent-guest` and `fxfs-guest-snapshot-dax`, ten rounds each. Boot as
the guest says it on its console; each restore's `/snapshot/load`, two a round.

| | Old (557b49a70) | New (56cdb1ecd) |
|---|---|---|
| boot, kernel start to init (10) | 275.1 ms (259-311) | 273.1 ms (259-307) |
| boot, to the guest agent (10) | 300.8 ms (277-337) | 296.9 ms (277-325) |
| a restore (20) | 26.4 ms (23.0-162) | 27.5 ms (25.2-372) |

Each side had one slow restore: 162 ms on the old side, 372 ms on the new. The other 19 on each
side took 23-31 ms.

On 0f011561a, `fxfs-guest-snapshot-dax` passed round 1 and timed out in round 2 (TASK-277). Both
restored guests printed `restored` and went idle. The host's listener waited in `accept4` for
each guest's new vsock connection, which never came. Root cause (TASK-277): a vsock connect answer
was left in the rx queue across the snapshot, and over PCI the reset was processed first. Its fix,
9d4676dd5 and 2adb9ccd6 (Firecracker fxdax-v1.16.1-r3), is what 56cdb1ecd's 20 rounds ran. The
failed run is in `boot-new/`; 0f011561a's boot figures from it match the table (275.2 ms,
296.6 ms).

## Perf gate

`scripts/perf-gate-gce.sh`: `single`, then `density`, then `gate`, every Firecracker jailed, a
release guest kernel. Old first in each phase. pgbench at scale 50: five 60 s runs of `-c 4 -j 2`;
the density cases run 3 VMs, two runs each.

| Case, tps median (range) | Old | New | New over old |
|---|---|---|---|
| fxfs-guest-pgbench | 1,436 (1,382-1,478) | 1,602 (1,549-1,641) | 1.12× |
| fxfs-guest-pgbench-dax | 1,407 (1,364-1,450) | 1,587 (1,565-1,639) | 1.13× |
| pgbench-ext4-dax-pmem | 1,196 (1,154-1,211) | 1,235 (1,209-1,284) | 1.03× |
| pgbench-ext4-blk-unsafe | 2,725 (2,591-2,782) | 3,258 (2,604-3,386) | 1.20× |
| pgbench-ext4-blk-writeback | 1,122 (1,102-1,166) | 1,260 (1,248-1,305) | 1.12× |
| fxfs-guest-pgbench-forked | 1,417 (1,189-1,432) | 1,594 (1,355-1,634) | 1.12× |
| fxfs-guest-pgbench-dax-forked | 1,267 (1,175-1,420) | 1,395 (1,307-1,577) | 1.10× |
| fxfs-guest-pgbench-density | 1,170 (1,141-1,191) | 1,352 (1,313-1,359) | 1.16× |
| fxfs-guest-pgbench-dax-density | 1,233 (1,197-1,302) | 1,331 (1,235-1,363) | 1.08× |

In every Fxfs case the guest's device writes and commits move from the 524 µs bucket of its
histogram at p50 to the 262 µs one (gate.json's `latency`). The device is a request faster, as
virtio-pci-vs-mmio measured: a write 121 µs to 73 µs.

| Gate | Old | New | Threshold |
|---|---|---|---|
| dax (DAX's tps over without) | 0.980 | 0.991 | at least 0.95 |
| forked-dax | 0.894 | 0.875 | at least 0.8 |
| decay, dax-forked | 1.078 | 1.183 | at least 0.9 |
| decay, dax-density | 1.086 | 1.104 | at least 0.9 |
| density, dax-density | 1.005 | 1.016 | at least 0.9 |
| density-memory, dax-density | 0.819 | 0.733 | at most 0.9 |
| density-memory-mib, dax-density | 2,015.8 | 2,015.6 | at most 2,560 |
| bigmap-read | 4.920 | **3.919** (failed) | at least 4.0 |
| bigmap-written-memory | 0.941 | 1.162 | at most 1.5 |

The other decay and density gates passed on both sides, within 0.04 of each other. Each run's
gates are in `gate-*/gate.json` and its runs step by step in `gate-*/steps.md`.

**bigmap-read (TASK-289).** It divides DAX's cold reads of a 4 GiB mapped file by those without
DAX. Both got faster over PCI, the reads without DAX more: each of their page faults is one device
read.

| bigmap | Old: no DAX | New: no DAX | Old: DAX | New: DAX |
|---|---|---|---|---|
| cold reads, MiB/s | 16.3 | 22.0 | 80.1 | 86.2 |
| warm reads, MiB/s | 21.8 | 30.1 | 19,333 | 19,305 |
| stores/s | 5,047 | 6,952 | 3,409 | 3,867 |
| store p50 / p99, µs | 218 / 503 | 152 / 455 | 277 / 439 | 252 / 369 |
| msync, s | 0.51 | 0.51 | 1.71 | 1.46 |

On PCI the gate is red on a speedup. TASK-289 re-measures its threshold on PCI or makes it
absolute.

**Memory per VM.** The host's `MemTotal - MemAvailable`, one density run each:

| Density case | First VM: old | new | Each VM more: old | new |
|---|---|---|---|---|
| without DAX | 3,659 MiB | 4,032 | 2,462 MiB | 2,750 |
| with DAX | 2,854 MiB | 3,071 | 2,016 MiB | 2,016 |

Without DAX, each VM more costs 288 MiB more on main, all of it the extent daemon's (2,122 to
4,710 MiB across the three VMs on the old side, 2,376 to 5,448 on the new). Each VM's process
holds 1,026 MiB on both. The daemon held more because it did more: 16% more transactions in the
same 60 s runs, so more of the WAL and part cache. restructure-before-after measured 2,572-2,790 MiB
a VM on main's trees then; 2,750 is within that. With DAX it is the same.

**Not run: pgbench-probes.** It belongs to scripts/dax-gate.sh. It measures a window MAP and
write(2) in pgbench-dax, and the perf gate's DAX window covers the whole database (TASK-227), so
it maps nothing: `window_maps 0`.

## Forks

**The fork bench cases.** test_fs_bench.rs's `bench-fork-*` on the n2-standard-4, three iterations,
jailed, as restructure-before-after ran them. A base VM copies `/usr` and makes a database; each
iteration forks it, boots a VM on the fork, reads `/usr` whole, rereads it, then runs pgbench `-S`
for 10 s. Figures: [forks.txt](perf-suite-pci-vs-before-2026-11-08/forks.txt). The old side's
first run lost five of its six cases to its root filesystem's build: wget timed out fetching Lua
from lua.org (TASK-297), as restructure's r910 did. They are rerun in `forks3-old-rerun/`, its
dax-profiled case in `forks3-old/`.

| Case | First read of /usr, old | new | pgbench -S tps, old | new |
|---|---|---|---|---|
| fxfs-guest-dax-warm | 0.8 s (0.8-3.9) | 0.8 s (0.8-3.8) | 26,135 (18,419-26,539) | 26,644 (24,138-26,710) |
| fxfs-guest-dax-cold | 4.1 s (4.1-4.2) | 4.1 s | 22,939 (20,178-22,959) | 18,972 (17,983-23,397) |
| fxfs-guest-dax-profiled | 0.8 s | 0.9 s (0.8-0.9) | 26,719 (26,242-26,947) | 25,707 (20,506-26,942) |
| fxfs-guest-warm (no DAX window) | 3.0 s (2.9-4.2) | 2.7 s (2.7-3.8) | 20,246 (16,422-20,257) | 21,638 (21,399-21,653) |
| ext4-dax-pmem | 2.0 s | 2.1 s | 22,177 (19,574-24,887) | 23,184 (19,183-25,194) |
| ext4-pmem | 4.2 s | 4.2 s | 18,218 (17,765-18,407) | 20,816 (18,666-23,030) |

A reread takes 0.0-0.1 s, and a fork's mount and PostgreSQL's start take the same time on both
sides. pgbench `-S`'s 10 s runs vary as much between iterations as between trees: dax-cold's new
range contains the old one's, and the warm case without a window and the ext4 cases come out the
other way.

**A fork of a 10 GiB base.** `scripts/fork-time-gce.sh`, both trees in one run on one host. No VM
takes part.

| | Old | New |
|---|---|---|
| import of the base | 203.7 s | 202.7 s |
| the first fork | 57 ms | 57 ms |
| the four after | 37-39 ms | 36-39 ms |
| the fork's first open (`stat`) | 35-42 ms | 34-37 ms |

## Linux checkout

`linux-checkout-ext4`, `-fxfs`, `-fxfs-dax` (TASK-136.1) on the c3, one run each side, then
ext4 and Fxfs once more each side. Reports: `checkout*/report.md`. The phases that differ:

| | Old, runs 1 / 2 | New, runs 1 / 2 |
|---|---|---|
| ext4 checkout | 16.27 / 15.99 s | 13.13 / 15.42 s |
| Fxfs checkout | 36.99 / 37.05 s | 34.46 / 34.34 s |
| Fxfs-DAX checkout | 36.91 s | 33.32 s |
| ext4 read-cold | 7.94 / 7.54 s | 4.94 / 4.09 s |
| Fxfs read-cold | 20.77 / 21.45 s | 16.85 / 16.98 s |
| ext4 reopened read-cold | 12.46 / 13.32 s | 9.45 / 10.41 s |
| Fxfs reopened read-cold | 23.64 / 24.05 s | 19.91 / 20.65 s |
| sandbox read-cold, Fxfs | 23.17-23.55 s | 19.79-20.72 s |
| ext4 sync after the checkout | 1.51 / 2.04 s | 4.58 / 1.88 s |
| Fxfs reopened read-warm | 538 / 572 ms | 842 / 627 ms |

The DAX cases' cold reads, which DAX serves, are the same: 10.6 against 10.2 s, and reopened 4.3
against 4.2 s. ext4's sync and Fxfs's reopened warm read were slower in the new side's first run
only. Their second runs are within the old side's.

Statuses, mounts, the clone, publish, the shared volume's attaches and imports, and the hundred
volumes' imports and mounts are within a few percent either way.

## Against the history

- **The transport A/B** (virtio-pci-vs-mmio-2026-11-09, same c3 host, a day earlier, one tree
  toggled by FXDAX_VIRTIO_TRANSPORT). Main matches its PCI run, and 557b49a70 its MMIO run. Over
  their 81 shared case-and-workload pairs, the median ratio is 0.97 each way, their tenth to ninetieth
  percentiles 0.84-1.02, as one day's runs against another's do. The rest of the change (the
  Firecracker series r1 to r2, MMIO off in the kernel, fxdax's MMIO code gone) adds nothing the
  transport didn't:

  | Case | Workload | 11-09 MMIO | old | 11-09 PCI | new |
  |---|---|---|---|---|---|
  | ext4-guest-kernel | rand-read-cold | 14,385 | 14,487 | 30,118 | 29,547 |
  | fxfs-guest | read-small-cold | 6,672 | 6,377 | 8,354 | 8,244 |
  | fxfs-guest | create-fsync | 2,163 | 2,247 | 2,830 | 2,824 |
  | fxfs-guest-dax | fsync-append | 2,938 | 2,685 | 3,631 | 3,284 |

  Boot and restore were the same there too: 271 against 269 ms, 25.6 against 26.1 ms.

- **The fs bench history** (fs-bench-history.md, regenerated). ext4 on virtio-blk's rand-read-cold
  was 14,486 IOPS on 10-31 and every MMIO run since, 29,547 now. Fxfs's metadata and DAX workloads
  are where the history had them.

- **The perf gate** (restructure-before-after-2026-11-07, n2-standard-4 in us-east4-a). Its tree
  "after", 213504c4b, MMIO:

  | | After (MMIO), 2026-11-07 | Old | New |
  |---|---|---|---|
  | fxfs-guest-pgbench | 1,418 tps | 1,436 | 1,602 |
  | fxfs-guest-pgbench-dax | 1,475 tps | 1,407 | 1,587 |
  | pgbench-ext4-dax-pmem | 1,155 tps | 1,196 | 1,235 |
  | forked-dax gate | 0.934 | 0.894 | 0.875 |
  | bigmap-read gate | 5.22 | 4.92 | 3.92 |

  The old tree is where the history is. PCI moved pgbench, and bigmap-read's baseline.

- **The fork bench** (same doc). Its "after" read /usr on a cold fork in 3.6 s; here both trees took
  4.1 s, on another host (us-east1-b against us-east4-a) and a week of main later, so not the
  transport.

- **A fork of a 10 GiB base** (fork-of-a-10-gib-base-2026-11-07, n2-standard-4 in us-east4-a). Forks
  took 27-29 ms then, 36-39 ms here on both trees (us-west1-a), and the import 221 s against 203 s.
  The host differs, and both trees agree, so this is not the transport.

## Not run

- `scripts/gets-gce.sh` and `scripts/example-gce.sh` (extentd-mapbench) measure GCS's ranged GETs
  and the daemon's requests with no VM, which the transport cannot change.
- `scripts/migrate-gce.sh` needs two hosts. Its restore is the snapshot restore above.
- `scripts/soak-gce.sh` and `soak-workloads-gce.sh` are hours-long soaks, not benchmarks.
- pgbench-probes, as above.

## Conclusions

Main on virtio-PCI is as fast as main before it or faster in every suite. Its gains are those of
the transport alone (virtio-pci-vs-mmio), no smaller: a device request is about 40% shorter, which
shows wherever the guest waits on one.

- Cold reads on virtio-blk 1.7-2.0×, on Fxfs without DAX 1.2-1.3×.
- fsyncs and creates with fsync 1.1-1.3×.
- pgbench 8-20%.
- A Linux checkout 4-19% faster, and its cold reads 14-46%.

What the guest or the DAX window serves is unchanged, as are boot, restore and forks.

The suite also turned up:

- TASK-277: a hang after a PCI restore. Fixed.
- TASK-289: a gate whose baseline PCI moved.
- TASK-290: stat-warm's slow mode, twice as often on PCI.
- TASK-297: the bench root filesystem's dependence on lua.org.
