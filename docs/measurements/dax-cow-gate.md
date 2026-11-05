# Copy-on-write DAX's gate: throughput and memory per VM (TASK-164.5)

What `scripts/perf-gate-gce.sh` measures for copy-on-write DAX (backlog doc-13, "Performance and
density"), how to run it, and its thresholds. The first run's numbers go under "Runs" below; the
thresholds marked provisional are then set from them (the user, 2026-11-04: density targets come
from measurement).

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

`FXDAX_GATE_<NAME>` overrides each (dashes as underscores). Provisional ones are listed in
`gate.json`'s `provisional`.

| Gate | Of | Bound | Default |
|---|---|---|---|
| `dax` | median tps of `dax` against `no-dax`, database made in the run | at least | 0.95, provisional |
| `forked-dax` | the same, `dax-forked` against `no-dax-forked` (doc-13 expects 0.9-1.0) | at least | 0.9, provisional |
| `density-memory` | each DAX density case's host memory per added VM against no DAX's | at most | 0.8, provisional |
| `density-memory-mib` | each DAX density case's host memory per added VM, MiB | at most | 768, provisional |
| `bigmap-read` | each DAX bigmap case's cold random read MiB/s against no DAX's | at least | 1.0, provisional |
| `bigmap-guest-cache` | each DAX bigmap case's guest page cache grown over its cold reads, a share of the file | at most | 0.05, provisional |
| `bigmap-written-memory` | `bigmap-dax`'s `host_used` grown over its writes, per byte written | at most | 2.0, provisional |

`decay`, `static-dax` and `density` (TASK-131.5) hold as before, `decay` and `density` for every
guest Fxfs pgbench case. `dax` held writable DAX at 0.95, which it never met (0.51-0.61x).

## Runs

None yet. Record the first: date, host, revision, `gate.json`'s gates, `memory` and `bigmap`, and
the thresholds set from it.
