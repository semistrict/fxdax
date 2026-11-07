# The guest suites against a real bucket (GCS) and a file bucket, 2026-11-06

Every guest suite had run on `file://` or `memory://` buckets, while TASK-165.1-165.8 rebuilt the
storage engine. Here the same tree (main f81174090; r744 with TASK-209's fix on top) ran the e2e
suite and the fork, snapshot, suspend and migration cases against GCS and against a file bucket.

**Setup.** `scripts/test-gce.sh`, n2-standard-4, `FXDAX_FIRECRACKER_JAIL=1` with the production
seccomp filter, release guest kernel built on the host, `FXDAX_WORKSPACE_TESTS=0`. GCS:
`FXDAX_GCE_STORAGE=1 FXDAX_EXTENTD_REMOTE=gs://echophase-fxdax-bench/real-bucket-rNNN` (the
bucket is in us-east4; the default compute service account's storage-rw scope sufficed, no IAM
change). File: test-e2e.sh's default, a file bucket on tmpfs. The cases that start daemons of
their own (`spawn_daemon`) keep their stores in `FXDAX_EXTENTD_REMOTE`'s bucket when the runner
has it, the GCS runs; otherwise in their daemon's memory, the file run.

| Run | Bucket | Zone | Cases |
|---|---|---|---|
| r740 | GCS | us-east1-c | `--tag e2e` and live-fork, snapshot, snapshot-dax, suspend-dax, migrate-dax, migrate-tenant-dax, pgbench-dax-forked; stopped at its 100 min limit before docker-dax and extent-guest |
| r741 | file | us-central1-a (us-east1 out of CPU quota) | the same, all |
| r742 | GCS | us-east1-c | docker-dax, extent-guest, checks and checks-dax with `FXDAX_CHECKS_SCALE=4` |
| r743 | GCS | us-central1-a | pgbench-dax-forked, again |
| r744 | GCS | us-east1-c | pgbench-dax-forked with TASK-209's fix |

## Each case

Times are the runner's (`round-1.json`'s `took_secs`), against the case's limit. The checks'
limits double with any `FXDAX_EXTENTD_REMOTE` (test_fxfs_guest_checks/host.rs:83-84).

| Case | GCS r740 | file r741 | GCS / file | GCS reruns |
|---|---|---|---|---|
| fxfs-guest | pass 7.1 s of 15 | pass 2.2 s of 15 | 3.23x |  |
| fxfs-guest-crash | pass 5.2 s of 15 | pass 3.1 s of 15 | 1.67x |  |
| fxfs-guest-live-fork | pass 5.8 s of 90 | pass 1.9 s of 90 | 3.05x |  |
| fxfs-guest-pgbench | pass 396.4 s of 1200 | pass 391.9 s of 1200 | 1.01x |  |
| fxfs-guest-pgbench-dax | pass 397.9 s of 1200 | pass 391.7 s of 1200 | 1.02x |  |
| fxfs-guest-pgbench-dax-forked | fail 1353.3 s of 1800 | pass 1276.9 s of 1800 | 1.06x | r743: fail 1386.5 s of 1800; r744: pass 1547.8 s of 1800 |
| fxfs-guest-snapshot | pass 9.0 s of 120 | pass 4.5 s of 120 | 2.00x |  |
| fxfs-guest-snapshot-dax | pass 8.5 s of 120 | pass 4.5 s of 120 | 1.89x |  |
| fxfs-guest-suspend-dax | pass 11.6 s of 120 | pass 5.5 s of 120 | 2.11x |  |
| fxfs-guest-migrate-dax | pass 9.2 s of 120 | pass 4.4 s of 120 | 2.09x |  |
| fxfs-guest-migrate-tenant-dax | pass 10.4 s of 120 | pass 4.9 s of 120 | 2.12x |  |
| fxfs-guest-checks | fail 184.8 s of 552 | pass 61.5 s of 291 | 3.01x | r742: pass 179.7 s of 2118 |
| fxfs-guest-checks-dax | fail 207.6 s of 716 | pass 76.5 s of 373 | 2.71x | r742: pass 212.7 s of 2774 |
| fxfs-guest-checks-dax-delay | fail 207.7 s of 716 | pass 80.0 s of 373 | 2.60x |  |
| fxfs-guest-checks-dax-small | fail 204.5 s of 706 | pass 79.3 s of 368 | 2.58x |  |
| fxfs-guest-checks-overrun | pass 174.5 s of 564 | pass 69.9 s of 297 | 2.50x |  |
| fxfs-guest-dax | pass 6.5 s of 15 | pass 2.4 s of 15 | 2.71x |  |
| fxfs-guest-crash-dax | pass 5.4 s of 15 | pass 4.4 s of 15 | 1.23x |  |
| fxfs-guest-crash-dax-killed | pass 5.1 s of 15 | pass 4.0 s of 15 | 1.27x |  |
| fxfs-guest-crash-stray-dax | pass 6.4 s of 15 | pass 5.0 s of 15 | 1.28x |  |
| fxfs-guest-docker-dax | not reached | pass 45.3 s of 300 |  | r742: pass 242.1 s of 300 |
| extent-guest | not reached | pass 0.7 s of 15 |  | r742: pass 1.8 s of 15 |

## The failures

- **fxfs-guest-checks, -checks-dax, -checks-dax-delay, -checks-dax-small** (r740): checks killed
  at their budgets, nothing else wrong. With 4x the budgets (r742) every check passes. torture
  took 81.6 s and 92.6 s against 16.5 s and 17.0 s on the file bucket; unbind_waits_for_unmount
  8.3 s against 0.84 s; append_writeback 8.6 s against 1.05 s; file_attributes 3.8 s against
  0.21 s; cow_store_persists 2.0 s (r740: 10.8 s) against 0.35 s. The guest's fsync and syncfs
  park in `fxfs_rt_park` while a durable commit waits for its upload and acknowledgement
  (crates/fxdax/src/extents.rs:1255, `make_durable`). TASK-165.11.
- **fxfs-guest-pgbench-dax-forked** (r740, r743): after `pg_ctl -m immediate stop`, the restart
  went past pg_ctl's 900 s. Its daemon added 25 ms to every request of the real bucket's
  (FXDAX_REMOTE_LATENCY_MS's default, meant for a bucket in memory), and the restart's reads of
  what the fork inherited were ranged GETs one after another. Fixed (TASK-209); r744 passes, the
  restart 97.9 s. Its balance sum after the restart still takes 927 s (689 s on the file bucket),
  the case 1,548 s of 1,800: TASK-165.12.
- Nothing that depends on a real bucket's semantics failed: no conditional write, listing or
  lease error, no 5xx or throttling in any daemon's log. Ordered commits stalled on the bytes not
  yet uploaded (`Options::max_lag_bytes`; remote.rs's "commits wait for object storage: 0 not
  uploaded", its count epochs, not bytes) 5 times in r740 and 2 in r742, once on the file bucket.

## The daemons' requests

The main daemon (test-e2e.sh's), r740 against r741, the same 20 cases, time per request:

| Request | GCS | file | x |
|---|---|---|---|
| open | 884 ms | 15.1 ms | 58 |
| fork | 697 ms | 25.7 ms | 27 |
| fault | 28.2 ms | 0.31 ms | 91 |
| read | 2.20 ms | 0.076 ms | 29 |
| commit | 23.4 ms | 4.51 ms | 5.2 |
| write | 1.35 ms | 0.35 ms | 3.9 |
| commits' wait for the bucket (`commit-wait-ns` / `commits`) | 3.60 ms | 0.56 ms | 6.4 |
| free, map, unmap, private, root, info | the same | | 1.0-1.3 |

pgbench-dax-forked's own daemon: faults 5.9 ms (r740) and 7.6 ms (r743) with the 25 ms added,
0.59 ms without (r744); 0.40 ms on its memory bucket with the 25 ms (r741).

## Bucket requests

From `extentd-stats.txt` (`extent_remote::store::Stats::line`), over each daemon's whole run; the
daemon reports no per-case counts.

| Daemon | get | get-range | list | create | update | delete | up segments | up extents | up checkpoints | down |
|---|---|---|---|---|---|---|---|---|---|---|
| r740 main, GCS (20 cases) | 3,709 | 11,306 | 208 | 7,606 | 15 | 1 | 412 MB | 1.8 MB | 1.1 MB | 826 MB |
| r742 main, GCS (4 cases) | 1,545 | 6,774 | 48 | 3,510 | 5 | 1 | 230 MB | 0.95 MB | 0.51 MB | 1.7 GB |
| r741 main, file (22 cases) | 214 | 3,669 | 124 | 218 | 9 | 1 | 118 MB | 7 KB | 36 KB | 1.9 GB |
| pgbench-dax-forked, r740 GCS + 25 ms | 358 | 16,924 | 41 | 575 | 3 | 1 | 384 MB | 10 MB | 2.5 MB | 1.2 GB |
| pgbench-dax-forked, r743 GCS + 25 ms | 347 | 12,737 | 38 | 538 | 3 | 1 | 390 MB | 5.7 MB | 1.3 MB | 0.91 GB |
| pgbench-dax-forked, r744 GCS | 487 | 28,402 | 48 | 859 | 3 | 1 | 513 MB | 18 MB | 3.5 MB | 1.9 GB |
| pgbench-dax-forked, r741 memory + 25 ms | 547 | 35,971 | 47 | 769 | 3 | 1 | 543 MB | 18 MB | 4.0 MB | 2.3 GB |

The file run's main daemon counts only what it put in the host's bucket: there, the stores the
cases make and fork keep their branches in buckets of their own, which it doesn't count
(TASK-165.6), so its row isn't the GCS row's equal. The forked case downloads about four times
what it uploads: no part cache (TASK-165.12).

## The collector

No guest case or script runs `fxdax-gc` against the run's bucket (scripts/test-rustfs.sh runs it
against a local RustFS alone), so no collector ran beside the suites' stores. After r740,
`fxdax-gc gs://.../run-20261106T064130Z-59e2abec --branches` listed 40 live branches, every one
a store the cases left; no host held them any more. Each run's prefix was deleted afterwards.

## The balance sum after the restart, again (TASK-165.12, 2026-11-07)

fxfs-guest-pgbench-dax-forked alone, main b57d57e4a, n2-standard-4 in us-east4-b, jailed, release
guest kernel. Since 800e9e140 (TASK-164.12) every test daemon keeps a part cache of its own, as
production's does, which r740-r744's had not; r812 turns it off again (`FXDAX_EXTENTD_CACHE=`
empty, its run started on the host by hand, as test-gce.sh passes no empty variable).

| Run | Bucket | Part cache | Case | The sum after the restart | Ranged GETs | Downloaded | Page-ins read |
|---|---|---|---|---|---|---|---|
| r744 | GCS | none | 1,547.8 s of 1,800 | 926.6 s | 28,402 | 1.9 GB | |
| r741 | memory + 25 ms | none | 1,276.9 s | 688.6 s | 35,971 | 2.3 GB | |
| r812 | memory + 25 ms | none | 1,182.0 s | 705.7 s | 36,290 | 2.31 GB | 2.86 GB |
| r810 | GCS | the daemon's | 432.3 s (24%) | 7.1 s | 18 | 1.2 MB | 4.60 GB |
| r811 | memory + 25 ms | the daemon's | 467.1 s (26%) | 14.8 s | 6 | 0.1 MB | 4.47 GB |

**What the sum waited on** (r812, the case daemon's samples over the sum's 660 s): 24,570
page-ins, each a ranged GET of the bucket (24,614), one after another, 26.8 ms each: the 25 ms the
case adds to its bucket's requests and the GET itself. A page-in of an extent the store keeps only
in the bucket reads it through `remote.read` (crates/fxdax/src/cache.rs:418-424), with no part
cache a GET each time (`extentd::stores::remote_cache`, crates/fxdax/src/extentd/stores.rs:1251),
again after each eviction from the extent cache: 1.35 GB downloaded for the 743 MB the page-ins
read. Faults 6.8 s of it, MAPs 5.1 s; the guest's vCPUs busy 8.7 s of their 1,320.

With the part cache (r810, the same window): 10,082 page-ins reading 204 MB of the bucket's data,
every one from the cache, no GET; uploaded segments join the cache as they are acknowledged
(TASK-165.8), so a fork's base, made on the host, is local. The case's daemon downloaded 1.2 MB in
all, against 4.6 GB its page-ins read.

Not changed, and production's: a host whose cache holds none of a fork's data, as one forking
another host's base, pages it in as r812 did, a ranged GET a page-in, one after another.
