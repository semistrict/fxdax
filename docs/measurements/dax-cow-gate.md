# Copy-on-write DAX's gate: throughput and memory per VM (TASK-164.5)

What `scripts/perf-gate-gce.sh` measures for copy-on-write DAX (backlog doc-13, "Performance and
density"), how to run it, its thresholds, and its runs: the first, r632, set the thresholds that
were first guesses (the user, 2026-11-04: density targets come from measurement).

## Running it

```sh
scripts/test-gce.sh create fxdax-test-perf-r1     # n2-standard-4, nested, by default
results=/tmp/perf-gate-$(date -u +%Y-%m-%dT%H%MZ)
scripts/perf-gate-gce.sh fxdax-test-perf-r1 "$results" single
scripts/perf-gate-gce.sh fxdax-test-perf-r1 "$results" density
scripts/perf-gate-gce.sh fxdax-test-perf-r1 "$results" gate
scripts/test-gce.sh delete fxdax-test-perf-r1
```

It runs every case below, one VM each (`RESULTS/single`), then the density cases
(`RESULTS/density`), then `scripts/pgbench-gate.py` over both, which writes `RESULTS/gate.json` and
fails on any gate. Without phases it runs all three in one command, more than two hours on an
n2-standard-4: r580 met its driver's two-hour limit in the density run. One phase a command keeps
each under it, the density run reusing the single run's builds on the host.

- Each Firecracker runs in its jailer's jail, as production runs it (`FXDAX_FIRECRACKER_JAIL=1`,
  set empty for none): since TASK-172, a jailed VM process's window budgets its mappings by the
  host's `vm.max_map_count`, which the daemon tells it, as an unjailed one's does.

- DAX copies on write, the only DAX since writable DAX was deleted (2026-11-04,
  dead-ends/writable-dax.md): the `-dax` cases are copy-on-write DAX's, and the `-dax-cow` cases
  and `fxfs.dax_cow` are gone.
- `FXDAX_PERF_GATE_VMS` (3) and `FXDAX_PERF_GATE_DENSITY_RUNS` (2): the density run's VMs and each
  one's runs.
- One case alone: `FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-bigmap-dax
  scripts/test-gce.sh run INSTANCE RESULTS`, then `scripts/pgbench-gate.py RESULTS`.

## The cases

| Case | What | Outputs |
|---|---|---|
| `fxfs-guest-pgbench`, `-dax` | pgbench (scale 50, five 60 s runs of `-c 4 -j 2`), each VM making its database: created files, which copy-on-write DAX keeps in the page cache | tps by run (`tps`) |
| `fxfs-guest-pgbench-forked`, `-dax-forked` | The base's VM makes and loads the database and stops PostgreSQL cleanly; the VM on a fork of the base runs pgbench on it: files read in, which DAX maps, every first store to a page a copy on the host | tps by run |
| `fxfs-guest-pgbench-density`, `-dax-density` | As `-forked`, with `FXDAX_PERF_GATE_VMS` VMs of 1 GiB on forks of the one base, one daemon serving all; each VM, its runs done, stays up (PostgreSQL running) until the last's are; the host's memory is measured before the first VM and after each VM's runs | tps by VM; `memory`: host memory per added VM and its parts |
| `fxfs-guest-bigmap`, `-dax` | The user's 400 GB file, scaled: a base's 4 GiB file (four times the 1 GiB guest), mapped `MAP_SHARED` read-write by a VM on a fork; each page read once at random, then pages at random for 10 s, then 5% of the pages stored to at random, each store timed, then `msync`; every page checked, through the mapping and `pread` | `bigmap`: MiB/s of cold and warm random reads, faults per page, stores/s, store latency p50/p99/max, msync seconds, and the memory at each step |

The host's memory, measured as each guest asks over vsock (`guest/tests/test_cases/src/measure.rs`),
is in the case's log as `memory: vm=K phase=P NAME=VALUE...`:

- `host_used`: MemTotal less MemAvailable of the host: every VM, the daemon and all else, each page
  once, page cache left out. The gated measure.
- `vmm_rss`, `vmm_pss`, `vmm_anon` (the guest's RAM it touched), `vmm_shmem` (the daemon's pages its
  window maps): the VM process's.
- `extentd_pss`, and the daemon's metrics: `extentd_memory_bytes` (its arenas: every tenant's
  extents' pages, private pages among them), `extentd_window_pages`, `extentd_faults_total`,
  `extentd_fault_stores_total`, and any gauge of private pages it has.
- `guest_*`: the guest's MemTotal, MemAvailable, Cached, AnonPages, Shmem, pgfault, pgmajfault,
  and its Fxfs's counts of copy-on-write DAX (`guest_fxfs_*`: those of /proc/fs/fxfs naming
  `private` or `cow`, such as `window_privates`).

A density case's host memory per added VM is the mean of `host_used`'s growth over each VM after the
first; the first's (`first_vm_mib`) includes what all share, the base's pages in the daemon.

Don't measure memory with `FXDAX_GUEST_HUGE_PAGES=2M`: a huge-page pool is never available, so
`host_used` doesn't see the guests' RAM.

## Thresholds

`FXDAX_GATE_<NAME>` overrides each (dashes as underscores). Copy-on-write DAX's were first guesses,
set from the first run, r632 (below): `dax` holds doc-13's expectation; `forked-dax` the user's
decision (2026-11-06) that doc-13's one host round trip a page's first store, measured at 0.836 and
0.854 of no DAX (r650, TASK-164.10), is acceptable, its threshold 0.8 for a nested host's noise; the
rest are r632's numbers with room for that noise (the user, 2026-11-04: density targets come from
measurement).

| Gate | Of | Bound | Default | r632 |
|---|---|---|---|---|
| `dax` | median tps of `dax` against `no-dax`, database made in the run | at least | 0.95 | 0.995 |
| `forked-dax` | the same, `dax-forked` against `no-dax-forked` (doc-13 expected 0.9-1.0; r650 0.836, the user accepted 0.84, 2026-11-06) | at least | 0.8 | 0.573 |
| `density-memory` | each DAX density case's host memory per added VM against no DAX's | at most | 0.9 | 0.862 |
| `density-memory-mib` | each DAX density case's host memory per added VM, MiB | at most | 2560 | 2361 |
| `bigmap-read` | each DAX bigmap case's cold random read MiB/s against no DAX's | at least | 4.0 | 5.76 |
| `bigmap-guest-cache` | each DAX bigmap case's guest page cache grown over its cold reads, a share of the file | at most | 0.05 | 0.0 |
| `bigmap-written-memory` | `bigmap-dax`'s `host_used` grown over its writes, per byte written | at most | 1.5 | 1.29 |

`decay` and `density` (0.9) and `static-dax` (0, reported) (TASK-131.5) hold as before, `decay` and
`density` for every guest Fxfs pgbench case. `dax` held writable DAX at 0.95, which it never met
(0.51-0.61x).

## Runs

### r800-r807, 2026-11-06: where a fork's PRIVATE goes, and forked-dax past 0.9 (TASK-164.11)

GCE n2-standard-4 (nested, Cascade Lake), `fxdax-test-r800-trace` and `fxdax-test-r801-haltpoll`
(us-east4-a), main 58937524e and its changes, release guest kernel, every Firecracker jailed.

**Where a PRIVATE goes** (r802: dax-forked alone, under scripts/extent-transport.bt, whose tracing
cost the pair little: r800 0.832 traced, r794 0.836 not). A PRIVATE takes the guest 527 us; the VM
process waits 282 us for the daemon's reply, of which the daemon's own work is 66 us, the stage run,
the VM process making the run private, 116 (mmap 25 us on average, 64-128 us over pages installed,
whose TLBs every vCPU flushes; KVM_PRE_FAULT_MEMORY 20; UFFDIO_COPY 10; pread, UFFDIO_REGISTER,
munmap, mprotect a few each), and ~100 us the messages between them. The other ~245 us are the
transport's and the guest's: an MMIO exit costs 8-32 us of the host's time (5.3M of them in r800),
of which the interrupt's handling takes two to the VMM (virtio-mmio's status read and ACK), a HLT
and the wakeup; the threads' waits to run are 1-4 us mostly. Neither halt polling in the guest
(`cpuidle_haltpoll.force=1`, r801: 0.826) nor a guest spinning for a PRIVATE's reply
(`virtio_extent.spin_ns=600000`, r803: 0.843, a PRIVATE 432 us from 463) moved forked-dax.

**What it cost the fork**: not a PRIVATE's time alone but its count, and the inode's lock it held.
A window kept a quarter of its pages private at most (512 MiB of 2 GiB), the pgbench accounts
table's 640 MiB not, so pages went back to their extents and their next stores asked again: 309,608
PRIVATEs (r802), and a write(2)'s PRIVATE held the inode's lock, which the file's other writes
waited for as long as the PRIVATEs took (160 s of lock waits for 163 s of PRIVATEs).

| Run | Change | no-dax-forked | dax-forked | forked-dax | PRIVATEs | Lock waits |
|---|---|---|---|---|---|---|
| r800 | none (traced) | 1388 | 1156 | 0.832 | | |
| r804 | `private_max` half the window | 1442 | 1315 | 0.912 | 108,006 | 37.5 s |
| r806 | the same, full gate | 1448 | 1293 | 0.893 | 107,222 | 36.9 s |
| r807 | and a write's PRIVATE before its lock, full gate | 1425 | 1300 | 0.912 | 108,688 | 7.4 s |

The density phase with half the window private (r805): `density-memory` 0.823, 2,256 MiB a VM
added (r794: 2,097); r806 0.771 and 2,200.5; r807 0.811 and 2,164.0.

r807, the full gate: every case and every gate passes. no-dax 1406, 1410, 1473, 1440, 1437; dax
1406, 1409, 1472, 1460, 1430; ext4-dax-pmem median 1157; no-dax-forked 1215, 1383, 1425, 1438,
1444; dax-forked 1203, 1328, 1300, 1442, 1296. Gates: `forked-dax` 0.912, `dax` 0.995, `static-dax`
1.24, `decay` 0.99-1.19, `density` 0.99 and 1.01, `density-memory` 0.811 (no DAX 2,668.7 MiB a VM
added, DAX 2,164.0), `density-memory-mib` 2,164.0, `bigmap-read` 5.15, `bigmap-guest-cache` 0.0,
`bigmap-written-memory` 1.31. dax-forked's write(2)s: 108.5 s, 65.5 of them making pages private
before the lock, 7.4 waiting for it.

### r792-r794, 2026-11-06: the test daemons with a part cache (TASK-164.12); the full gate passes

The test daemons had no part cache, which production's daemon keeps on the host's disk
(`FXDAX_EXTENTD_CACHE`, doc-7): r790's fork paged what it inherited in from the bucket a ranged GET
at a time. Since TASK-164.12 each keeps one (guest/tests/test_cases/src/common.rs's `part_cache`,
scripts/test-e2e.sh's), unless `FXDAX_EXTENTD_CACHE` says otherwise, empty for none. GCE
n2-standard-4 (nested, Cascade Lake), main 76d2c39cf (vendor/firecracker 3d1e01bd2) with that
change, release guest kernel, every Firecracker jailed.

The forked pair alone first, as r790's: on `fxdax-test-r792-cache` (us-east4-a) at the gate's
25 ms, no-dax-forked 1217, 1353, 1458, 1506, 1510, dax-forked 1181, 1266, 1185, 1303, 1238:
`forked-dax` 0.849, `decay` 1.049, 26 ranged GETs (r790: 57,466); on `fxdax-test-r793-cache`
(us-central1-a) at 0 ms, `forked-dax` 0.864, `decay` 1.080, 16 ranged GETs (r790's rerun: 115,900).

Then the full gate (r794): the single phase on r792, the density phase on r793, the gate phase over
both. Every case passed, every gate:

| Case | Runs | Median |
|---|---|---|
| no-dax | 1440, 1450, 1502, 1519, 1488 | 1488 |
| dax | 1414, 1421, 1467, 1466, 1449 | 1449 |
| ext4-dax-pmem | 1189, 1202, 1224, 1261, 1246 | 1224 |
| no-dax-forked | 1208, 1384, 1450, 1459, 1503 | 1450 |
| dax-forked | 1163, 1221, 1188, 1287, 1211 | 1211 |
| no-dax-density (3 VMs, 2 runs) | 865, 957 / 968, 946 / 922, 951 | |
| dax-density (3 VMs, 2 runs) | 923, 974 / 1026, 986 / 974, 962 | |

Gates: `forked-dax` 0.836, `dax` 0.974, `static-dax` 1.18, `decay` 1.03-1.24 (dax-forked 1.04),
`density` 1.03 and 1.02, `density-memory` 0.847 (no DAX 2,476.8 MiB a VM added, DAX 2,097.4; first
VMs 3,103.3 and 2,564.4), `density-memory-mib` 2,097.4, `bigmap-read` 5.52 (93.6 against 16.9
MiB/s), `bigmap-guest-cache` 0.0, `bigmap-written-memory` 1.32 (bigmap with DAX: 2,850 stores/s,
p50 347 us, p99 467, max 1,015; msync 1.12 s).

dax-forked's daemon: 126,148 page-ins, 11 of them ranged GETs, the rest from the part cache; a
PRIVATE request 247 us on average (79.6 s over 321,910; r790: 6.4 ms over 160,488).

The dax-gate (scripts/dax-gate.sh) on r792 and r795, then r799 and r795 (us-east4-a), the first
since 3e6615852 (2026-10-29). It failed twice before passing, neither time on DAX's code:
`pgbench-probes.py`'s window MAP request, 2,202 us against its 560 us ceiling, and then the mutant
`inherited_acls_written_back`, missed. The probe took a MAP request's mean, set by writable DAX's
r365, and since TASK-164.10 a request holds ~38 maps: it now takes a map's mean, 49-71 us since
TASK-164.10 (r649: 226), with a ceiling of 80, half as much again as r794's 53. The mutant's killer,
the crash test, passed 15 rounds of 15 with it in: since TASK-199 a setxattr is made at once, so
ACLs set after their creation are durable by the next fsync all the same. Its killer is now
`inherited_acls_on_full_filesystem`: on a full filesystem, the creation is made and its ACLs refused.
At dde984800 the gate passes, noted on it: the six suites 5 rounds each; pgbench-dax-forked, its
data checked, a map 59 us, a write(2) 428 us; the simulator's mutants 35 killed, 6 `none`; the
real kernel's, task-212's and task-79.29's killed, task-79.49's `none`.

### r790 and r791, 2026-11-06: the full gate on main ccbe81d0e, jailed, nested; fails `forked-dax` and `decay` of `dax-forked`

GCE n2-standard-4 (nested, Cascade Lake): the single phase on `fxdax-test-r790-gate` (us-east4-a),
the density phase on `fxdax-test-r791-gate` (us-central1-a), both at ccbe81d0e (vendor/firecracker
3d1e01bd2), release guest kernel, every Firecracker jailed; the gate phase over both. Every case
passed; every gate but two:

| Case | Runs | Median |
|---|---|---|
| no-dax | 1390, 1379, 1472, 1474, 1481 | 1472 |
| dax | 1437, 1428, 1474, 1455, 1435 | 1437 |
| ext4-dax-pmem | 1169, 1143, 1169, 1192, 1192 | 1169 |
| no-dax-forked | 1055, 1370, 1444, 1467, 1470 | 1444 |
| dax-forked | 711, 1023, 439, 116, 81 | 439 |
| no-dax-density (3 VMs, 2 runs) | 782, 756 / 886, 759 / 894, 757 | |
| dax-density (3 VMs, 2 runs) | 636, 871 / 963, 886 / 974, 901 | |

Gates: **`forked-dax` 0.304 (fails 0.8)**, **`decay` of dax-forked 0.114 (fails 0.9)**; `dax` 0.976,
`static-dax` 1.23, `decay` 0.97-1.39 elsewhere, `density` 1.07 and 1.24, `density-memory` 0.871
(no DAX 2,409.5 MiB a VM added, DAX 2,097.8; first VMs 3,282.6 and 2,646.5), `density-memory-mib`
2,097.8, `bigmap-read` 5.32 (88.9 against 16.7 MiB/s), `bigmap-guest-cache` 0.0,
`bigmap-written-memory` 1.32 (bigmap with DAX: 2,780 stores/s, p50 355 us, p99 471, max 1,004; msync
1.14 s).

What dax-forked spent: its PRIVATEs' daemon part grew from 0.4 ms to 540 ms a request, the VM
process's runs staying at 0.1-0.4 ms: each waited for its source page to be brought in, and the
daemon paged in one part at a time, a ranged GET of the bucket each, ~2,200 a minute, one every
~27 ms, which the case's 25 ms of added latency a bucket request makes (its default, a bucket in
memory; TASK-209 keeps it off real buckets). Since TASK-165.8, acknowledged segments leave the
store for the bucket, so a fork reads what it inherited from there: r649 and r650 fetched nothing.
No DAX pays those reads once, into its guest's page cache; DAX pays them again whenever the daemon
must bring a page in. The same pair rerun on r790 with FXDAX_REMOTE_LATENCY_MS=0: dax-forked 1180,
1250, 941, 1082, 1088 against 1197, 1363, 1439, 1458, 1460, `forked-dax` 0.756 (fails 0.8),
`decay` 0.92, still 13-36k page-ins a minute, a ranged GET each, 400-1,100 MiB fetched, the
daemon's part of a PRIVATE 47-391 us (r650: 83). TASK-164.12 has it. The dax-gate (scripts/dax-gate.sh)
wasn't run.

### r650, 2026-11-06: single phase, jailed, nested; `forked-dax` 0.836, from r649's 0.579

It failed the 0.9 then in force; the user took it, 2026-11-06, and the threshold is 0.8 since. A rerun of
the forked pair on the same host, at e51d4ade6: 0.854 (1,216 against 1,424).

GCE `fxdax-test-r650-tperf`, n2-standard-4 (nested, Cascade Lake), us-east4-a, release guest
kernel, every Firecracker jailed. At fedc14fd5 with TASK-164.10's changes uncommitted (their
commits 99e68ad10 and f485ccb2f): a `PRIVATE`'s pages made as one run in the VM process, pages
made private ahead of a log's writes, rebinds a range at a time in one `MAP_MANY`. The density
phase wasn't run.

| Case | Runs | Median |
|---|---|---|
| no-dax | 1415, 1421, 1469, 1455, 1439 | 1439 |
| dax | 1430, 1426, 1464, 1437, 1439 | 1437 |
| ext4-dax-pmem | 1156, 1121, 1164, 1183, 1183 | 1164 |
| no-dax-forked | 1208, 1387, 1455, 1460, 1471 | 1455 |
| dax-forked | 1175, 1257, 1177, 1308, 1217 | 1217 |

Gates: `dax` 0.999, `static-dax` 1.23, `decay` 1.00-1.22, **`forked-dax` 0.836** (0.9 then),
`bigmap-read` 5.13, `bigmap-guest-cache` 0.0, `bigmap-written-memory` 1.32. bigmap with DAX: cold
reads 86.6 MiB/s (no DAX 16.9), stores 3,139/s, p50 315 us.

What dax-forked's second run spent, against r649's (TASK-164.10's notes): write(2)s 21.9 s a
minute (68.8), 6.6 s of it for the inode's lock (31.9); `PRIVATE`s 26,068 (62,146), 486 us each
guest-side, the daemon's part 85 us and the VM process's run of ~4 pages 137 us; rebinds' `MAP`
requests 655 (27,886); the vCPUs 84% busy (76%), no DAX's 91%.

### r632, 2026-11-05: jailed, nested; fails `forked-dax` and `decay` of `dax-density`

GCE `fxdax-test-r632`, n2-standard-4 (nested, Cascade Lake), us-east4-a. Every Firecracker in its
jailer's jail under the production seccomp filter, its DAX window budgeting 522,240 runs as the host
allows (TASK-172). Release guest kernel. Single run at 3309abe89 with TASK-172's change uncommitted
(3da9444bd's tree); density run at 0e09b9389 with b902ece56's change uncommitted. Each phase its own
command: single 2 h (its kernel build 13 min, cases 63 min, the fetch of 11 GB of results 35 min,
8 GB of it `pgbench-ext4-dax-pmem`'s pmem image), density 30 min.

The density run's first attempt failed `fxfs-guest-pgbench-density`: its three VMs, running at
once, booted from one root image, which each VM's boot made anew under those running on it; their
ext4 failed its checksums and PostgreSQL stopped. Each VM has an image of its own since b902ece56.

pgbench, tps by run (first VM; density: each VM's median):

| Case | Runs | Median |
|---|---|---|
| no-dax | 1430, 1433, 1467, 1484, 1499 | 1467 |
| dax | 1445, 1415, 1460, 1484, 1469 | 1460 |
| ext4-dax-pmem | 1156, 1126, 1159, 1177, 1169 | 1159 |
| no-dax-forked | 1221, 1382, 1459, 1486, 1469 | 1459 |
| dax-forked | 884, 789, 835, 797, 864 | 835 |
| no-dax-density (3 VMs) | | 1216, 1220, 1199 |
| dax-density (3 VMs) | | 839, 864, 844 |

Gates: `dax` 0.995, `static-dax` 1.26, `decay` 0.98-1.20 but `dax-density` 0.883 (fails 0.9),
`density` 0.987 and 1.005, **`forked-dax` 0.573 (fails 0.9)**. The same pair unjailed on the same
host: `forked-dax` 0.570 (dax-forked 891, 778, 827, 792, 886), so the jail costs nothing here.
Each dax-forked VMM restricted pages to reads for a KVM worker's write some 2^18 times or more
(its log has 1,009 such lines: the first thousand, then each power of two), each costing the vCPU a
failed `KVM_RUN` and a retry: the likeliest place the forked gap is.

Density, host memory (`host_used`, MiB) after each VM's runs:

| Case | Before | VM 1 | VM 2 | VM 3 | First VM | Per added VM |
|---|---|---|---|---|---|---|
| no-dax-density | 1683 | 5592 | 8270 | 11071 | 3908 | 2740 |
| dax-density | 1804 | 5006 | 7397 | 9727 | 3202 | 2361 |

Per VM, no DAX: the VMM 1,026 MiB (its guest's RAM), the daemon's memory +1,310 MiB (its extents'
pages); DAX: the VMM 2,200 MiB RSS of which 790 anonymous and 700 the daemon's pages it maps, the
daemon +820 MiB. DAX saves 14% a VM (0.862).

bigmap (4 GiB file, 1 GiB guest):

| | Cold read MiB/s | Warm MiB/s | Stores/s | Store p50 / p99 / max us | msync s | Guest cache share | Host bytes per byte written |
|---|---|---|---|---|---|---|---|
| no DAX | 16.8 | 21.8 | 5128 | 215 / 476 / 73,611 | 0.46 | 0.197 | 0.42 |
| DAX | 96.8 | 19,996 | 3031 | 333 / 445 / 980 | 1.22 | 0.0 | 1.29 |

Thresholds set from it: above.
