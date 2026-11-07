# The storage restructure, before and after: the perf gate, forks and restores (TASK-165.10)

TASK-165 (doc-14) made local disk the bucket's write-ahead log and part cache, with one recovery
and forks in the bucket alone. Here the perf gate the DAX work used (dax-cow-gate.md), and the fork
bench and snapshot cases, ran on main before the restructure, on main before its last step
(TASK-165.9), and on main after it, on hosts alike.

**Trees.**

- *Pre-restructure* (r914): main at aee7cad93, just before TASK-165.6, doc-14's step 6, the first a
  guest sees: local lineage, layers, the ledger, the index, claims and handles; a fork's data read
  from its base's local files.
- *Before 165.9* (r910, r912): main at 394fe7535, before TASK-165.9's commits (d4c16ce00 and
  those around it), TASK-165.6-165.8 landed: no local lineage, layers or claims; every store keeps
  its branch in a bucket, and acknowledged segments leave for the part cache; but the index, local
  checkpoints and deltas, the markers and remote.rs's `Plan` and `diff`.
- *After* (r911): main at 213504c4b, all of TASK-165.

**Hosts.** `scripts/test-gce.sh`, n2-standard-4 (4 vCPUs, 16 GiB), nested, us-east4-a and
us-east1-b (r912), kernel 7.0.0-1011-gcp, a release guest kernel built on each host, each
Firecracker jailed (`FXDAX_FIRECRACKER_JAIL=1`). Every daemon on a file bucket, the pgbench cases'
with 25 ms added to each request, as their default is. r910, r911 and r914, one host each:
`scripts/perf-gate-gce.sh` `single`, then the fork and snapshot cases, then `density` and `gate`.
r912 reran `bench-fork-fxfs-guest-dax-warm` on the tree before 165.9, which r910 skipped: its root
filesystem's build timed out fetching lua's source from lua.org.

Kept here: each run's `gate.json`, the fork cases' figures (`forks.txt`), r912's report and the
main daemon's stats over the fork and snapshot cases, in restructure-before-after-2026-11-07/.

## The perf gate

pgbench: scale 50, five 60 s runs of `-c 4 -j 2` (the density cases: 3 VMs, two runs each).

| Case, tps median (mean) | Pre-restructure | Before 165.9 | After |
|---|---|---|---|
| fxfs-guest-pgbench | 1,448 (1,436) | 1,424 (1,432) | 1,418 (1,429) |
| fxfs-guest-pgbench-dax | 1,463 (1,458) | 1,427 (1,431) | 1,475 (1,455) |
| pgbench-ext4-dax-pmem | 1,155 (1,158) | 1,167 (1,168) | 1,155 (1,157) |
| fxfs-guest-pgbench-forked | 1,419 (1,371) | 1,403 (1,351) | 1,401 (1,356) |
| fxfs-guest-pgbench-dax-forked | **826 (815)** | 1,288 (1,296) | 1,309 (1,326) |
| fxfs-guest-pgbench-density | 1,209 (1,203) | 1,172 (1,172) | 1,196 (1,195) |
| fxfs-guest-pgbench-dax-density | **836 (855)** | 1,258 (1,255) | 1,247 (1,250) |

| Gate | Pre-restructure | Before 165.9 | After | Threshold (main's) |
|---|---|---|---|---|
| dax (DAX's tps over without) | 1.010 | 1.002 | 1.040 | at least 0.95 |
| forked-dax | **0.582** (failed) | 0.918 | 0.934 | at least 0.8 (0.9 then) |
| decay, dax-forked (last run over first) | 0.942 | 1.090 | 1.059 | at least 0.9 |
| decay, dax-density | **0.878** (failed) | 1.079 | 1.121 | at least 0.9 |
| density-memory, dax-density (per added VM, over without DAX) | 0.880 | 0.820 | 0.738 | |
| bigmap-read (DAX's cold reads over without) | 5.56 | 5.01 | 5.22 | at least 4 |
| bigmap-written-memory | 1.24 | 1.25 | 1.18 | at most 1.5 |

Every gate passed before 165.9 and after; pre-restructure failed two, its forked DAX cases a third
slower: their forks read what they inherited through local layers over the base's files, which the
restructure replaced with the part cache (TASK-164.12 measured the same gap from the other side,
forked-dax 0.304 without a part cache and 0.849 with one).

**Memory per VM** (the host's `MemTotal - MemAvailable`, dax-cow-gate.md):

| Density case | First VM: pre | before 165.9 | after | Each VM more: pre | before 165.9 | after |
|---|---|---|---|---|---|---|
| without DAX | 3,770 MiB | 3,617 | 3,756 | 2,790 MiB | 2,744 | 2,572 |
| with DAX | 3,058 MiB | 3,020 | 2,793 | 2,455 MiB | 2,249 | 1,898 |

The daemon's memory with DAX (`extentd_memory_bytes`, after each VM) went from 1,694-3,408 MiB
pre-restructure to 944-980 before 165.9 and 928-988 after: a fork's pages no longer kept twice, its
layers' and its base's. A VM more costs the host 557 MiB less with DAX than pre-restructure, 218
less without.

**bigmap** (a 4 GiB file in a base, mapped shared by a VM on a fork, read all over, 5% written):

| | without DAX: pre | before 165.9 | after | with DAX: pre | before 165.9 | after |
|---|---|---|---|---|---|---|
| cold reads, MiB/s | 16.5 | 16.3 | 16.9 | 91.9 | 81.4 | 88.2 |
| stores/s | 5,202 | 4,989 | 5,163 | 3,132 | 2,834 | 3,437 |
| store p50 / p99, us | 214 / 517 | 218 / 551 | 211 / 531 | 324 / 464 | 353 / 492 | 270 / 445 |
| msync, s | 0.43 | 0.43 | 0.46 | 1.09 | 1.13 | 1.14 |
| host written, MiB | 105 | 101 | 49 | 253 | 256 | 242 |

## Forks, restores and first reads

**Fork and restore latency.** The main daemon's per-request times over the fork and snapshot cases
(`extentd-stats.txt`'s `requests:`, their counts in brackets), and each restore's `/snapshot/load`
as the snapshot cases time it (pre-restructure's cases didn't say yet):

| | Pre-restructure | Before 165.9 | After |
|---|---|---|---|
| fork (ForkStore and the extent device's FORK) | 18.3 ms (14) | 26.3 ms (11) | 12.0 ms (14) |
| open | 19.4 ms (32) | 10.9 ms (24) | 7.6 ms (32) |
| commit | 1.40 ms (89) | 4.95 ms (113) | 2.41 ms (143) |
| fxfs-guest-snapshot-dax's two restores | | 45.7, 47.3 ms | 33.4, 38.7 ms |
| fxfs-guest-migrate-dax's restore, from the bucket | | 50.2 ms | 28.6 ms |

A fork waits for its commit's upload and acknowledgement and the child's record in all three;
pre-restructure it made a local fork too (a claim, a record, layers, a handle), and before 165.9 its
child took its state through the index; after, the child's directory holds only its description,
its state recovered from the bucket as it opens.

**First read of /usr and cold reads** (test_fs_bench.rs's fork cases: a base VM copies the guest's
`/usr` into its store and makes a PostgreSQL database; each of three iterations forks the base and
boots a VM on the fork that reads the tree whole, again, and runs pgbench's read-only
transactions for 10 s; medians, the first fork's in brackets where it differs):

| Case, first read of /usr | Pre-restructure | Before 165.9 | After |
|---|---|---|---|
| dax-warm (the base's daemon) | 0.7 s (3.0) | 0.8 s (3.1) | 0.8 s (3.5) |
| dax-cold (a daemon of its own, nothing cached) | 3.1 s | 3.3 s | 3.6 s |
| dax-profiled (cold, the base's boot profile read in) | 0.7 s | 0.8 s | 0.8 s |
| warm, no DAX window | 2.8 s | 3.0 s | 2.8 s |

| Case, pgbench -S tps | Pre-restructure | Before 165.9 | After |
|---|---|---|---|
| dax-warm | 22,870 (19,115-27,093) | 27,184 (24,196-27,693) | 24,486 (24,317-27,201) |
| dax-cold | 18,624 | 18,335 | 19,043 |
| dax-profiled | 27,468 | 26,515 | 27,122 |
| warm, no DAX window | 21,335 | 20,664 | 21,524 |

A reread of /usr takes 0.0-0.1 s in every case. pgbench -S's 10 s runs vary as much between
iterations as between trees.

**The cold fork is slower.** Pre-restructure, its daemon read the fork's data from the base's local
files, through the handle (fetched 0 bytes); before 165.9 and after, it fetches it from the bucket,
626-628 MB a fork, through a part cache of its own, empty. Its guest's device reads took 3.5-3.8 s
pre-restructure, 4.3-4.4 s before 165.9 and 5.7-5.9 s after; each of ~4,150 faults 0.29 ms,
0.31 ms and 0.40 ms; the first read of /usr 3.1 s, 3.3 s and 3.6 s. So too the warm case's first
fork, whose daemon hadn't read the base's data back yet: its faults 0.27 ms, 0.24 ms and 0.33 ms.
Each of three iterations alike, so past noise: the price of a fork whose data is in the bucket on a
host that holds none of it, which doc-14 accepted (forks only in the bucket) and TASK-225 measures
and works on (below).

## Bucket requests per workload

Over each case's daemon's whole run (`extent_remote::store::Stats`), pre-restructure / before
165.9 / after:

| Daemon | get | get-range | create | down |
|---|---|---|---|---|
| fxfs-guest-pgbench | 593 / 577 / 471 | 5 / 6 / 11 | 3,474 / 4,100 / 3,121 | 15 KB / 15 KB / 0.9 MB |
| fxfs-guest-pgbench-dax | 587 / 576 / 444 | 5 / 10 / 11 | 3,271 / 4,010 / 3,469 | 15 KB / 0.3 MB / 0.9 MB |
| fxfs-guest-pgbench-forked | 538 / 533 / 449 | 5 / 8 / 15 | 3,744 / 4,565 / 3,423 | 66 KB / 0.3 MB / 1.3 MB |
| fxfs-guest-pgbench-dax-forked | 563 / 541 / 446 | 5 / 14 / 21 | 2,178 / 2,550 / 4,710 | 68 KB / 0.7 MB / 1.6 MB |
| the main daemon, fork and snapshot cases | 357 / 334 / 400 | 59 / 1,307 / 2,352 | 3,096 / 2,399 / 3,242 | 1.6 / 230 / 446 MB |

Each pgbench case uploads about 1 GB of segments in all three; after, it reads a little more back
(GETs of tails as a fork's child recovers, under 2 MB). The main daemon's downloads grew with the
restructure: its forks' and restores' data is read from the bucket, into the part cache, where
pre-restructure it was read from local files (and before 165.9's run had 3 forks fewer, its
dax-warm case skipped). The cold and profiled cases' daemons count no request of their own: their
stores keep their branches in the base's bucket, whose requests the stats of the daemon whose
bucket it is count.

## What it says

Against the tree before the restructure, the restructure made forked DAX 58% faster
(1,309 tps against 826, its two failing gates passing), a VM cheaper for the host (557 MiB less
each with DAX), forks faster (12 ms against 18) and opens faster (7.6 ms against 19.4); pgbench
without forks, density without DAX and bigmap are the same. Against the tree before TASK-165.9
alone, nothing regressed past noise in the gate, and forks (12 against 26 ms), restores (29-39
against 46-50 ms), opens and commits are faster. The one regression is a cold host's page-ins of
a fork's data, which pre-restructure read from local files and now fetches: the first read of /usr
3.6 s against 3.1, faults 0.40 ms against 0.29. TASK-225 owns it; on pgbench-dax-forked with a
cold daemon its first run is 17% slower than a warm one's (TASK-225's notes).
