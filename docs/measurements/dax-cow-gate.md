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
set from the first run, r632 (below): `dax` and `forked-dax` hold doc-13's expectations; the rest
are r632's numbers with room for a nested host's noise (the user, 2026-11-04: density targets come
from measurement).

| Gate | Of | Bound | Default | r632 |
|---|---|---|---|---|
| `dax` | median tps of `dax` against `no-dax`, database made in the run | at least | 0.95 | 0.995 |
| `forked-dax` | the same, `dax-forked` against `no-dax-forked` (doc-13 expects 0.9-1.0) | at least | 0.9 | 0.573 |
| `density-memory` | each DAX density case's host memory per added VM against no DAX's | at most | 0.9 | 0.862 |
| `density-memory-mib` | each DAX density case's host memory per added VM, MiB | at most | 2560 | 2361 |
| `bigmap-read` | each DAX bigmap case's cold random read MiB/s against no DAX's | at least | 4.0 | 5.76 |
| `bigmap-guest-cache` | each DAX bigmap case's guest page cache grown over its cold reads, a share of the file | at most | 0.05 | 0.0 |
| `bigmap-written-memory` | `bigmap-dax`'s `host_used` grown over its writes, per byte written | at most | 1.5 | 1.29 |

`decay` and `density` (0.9) and `static-dax` (0, reported) (TASK-131.5) hold as before, `decay` and
`density` for every guest Fxfs pgbench case. `dax` held writable DAX at 0.95, which it never met
(0.51-0.61x).

## Runs

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
