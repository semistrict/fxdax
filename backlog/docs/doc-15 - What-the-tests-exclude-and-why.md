---
id: doc-15
title: 'What the tests exclude, and why'
type: specification
created_date: '2026-11-07 13:38'
updated_date: '2026-11-07 13:38'
---
# What the tests exclude, and why (TASK-149.20, 2026-11-07)

Every place a test or suite skips, ignores or filters, with its reason and verdict. `scripts/check-exclusions.py` (scripts part, CI) keeps the reasons from here on: an ignored Rust test of fxdax's says why, a known-failure line names an open task, an xfstests known failure sits under a comment saying why, a `Killer: none` mutant says why.

Verdicts: **kept** (still justified, reason named), **fixed** (stale: runs again, what failed fixed), **filed** (a gap, a task for it).

## Ignored Rust tests (`#[ignore]`)

| Test | Why | Verdict |
|---|---|---|
| extent_remote `bench_passes_over_many_forks`, `bench_reads_while_parts_are_kept`, `bench_sequential_reads`, `bench_reads_of_the_cache_on_a_runtime`, `bench_reads_of_parts_in_memory_by_a_hosts_fetchers`, `client_throughput`; fxdax `bench_sustained_uploads`; extent_frame `bench_checking_parts`; extent_store `hot_paths`; fxfs `layer_read_cost` | Benchmarks: run alone, some in release, with `--run-ignored only` | kept; each now `#[ignore = "a benchmark (TASK-…)…"]` |
| extent_remote `sim::one` | Debugs one seed (`SIM_SEED`); the sim part runs the soaks and searches | kept, reason added |
| extent_remote `store::tests::rustfs`, fxdax `rustfs_branches_survive_their_hosts_loss`, `rustfs_forks_and_takes_branches`, `rustfs_places_and_syncs_stores` | Need an S3 server | kept, reason added; **fixed**: nothing ran them. `rustfs_places_and_syncs_stores` failed: a real bug, `fxdaxctl take DIR CAP BRANCH TOKEN` of a new copy took the branch's lease with the host's token before the placement's, so a later placement couldn't take a branch over (`ESTALE`); fixed in `ExtentStore::take_with`. The two branch tests now also run in every suite run on a file bucket; CI's new object-stores job runs the S3 ones against RustFS 1.0.4 in a container (scripts/test-rustfs.sh, which ran `scripts/test.sh` with `--run-ignored only`, every part, and needed a RustFS built from source) |
| extent_remote `store::tests::azurite` | Needs Azurite | kept, reason added; **fixed**: CI's object-stores job runs it (scripts/test-azurite.sh, which took a podman installed but not running, and whose readiness probe gave up on an empty reply) |
| extent_remote `store::tests::gcs` | Needs a GCS bucket and credentials | kept, reason added; **filed** TASK-149.20.5 (runs nowhere) |
| fxdax `import::child_dies_importing`, extentd `child_maps_an_extent`, `child_changes_its_window`, `child_leaves_its_socket_to_a_child`, `child_opens_a_store` | Child processes of other tests | kept (named already) |
| fuchsia-async `test_support` ×2 (should_panic under LSAN, fxbug 42138715), `runtime` cfg_attr(asan) ignore; fuchsia_merkle `test_very_large_verification`; fxfs `test_seek_is_log_n_complexity` (flaky upstream) | Upstream Fuchsia's, in Fuchsia's files | kept: upstream's ways (the check exempts Fuchsia's files) |

## Filters, cfg gates, parts

| What | Why | Verdict |
|---|---|---|
| scripts/test.sh workspace part `-E "not (package(extent_remote) & test(/^sim::/))"` | The simulation is a part of its own, optimized (TASK-188) | kept |
| firecracker part `-E 'test(/extent::/) \| test(/vcpu::tests::(efaults_\|test_exit_code_)/)'` | The VMM's tests fxdax adds to; the rest are upstream's, some need KVM | kept |
| shuttle part: extent_store, physmem, fxdax, follower, follower_sim | Every crate whose Shuttle tests exist; fuchsia-async, fuchsia-sync and fxfs have the feature as plumbing alone | kept |
| hunt.yml's `-E` filters | Hunting jobs aim at the model and corrupt tests, and one test's seeds | kept |
| No nextest config, no default-filter | — | — |
| `#![cfg(target_os = "linux")]` on 12 fxdax integration tests (extentd, FUSE, seccomp…) | Linux-only features; CI runs Linux | kept |
| macOS-only code and tests (physmem's physmap, extent_store dir.rs syncs, extent_remote cache, storage io.rs) | CI runs no macOS | **filed** TASK-149.20.2 |
| Features gating tests: `shuttle` (its part), `audit`, `failpoints`, `testing` (enabled by dev-dependencies); `migration`, `vdso_next`, `kernel`, `tracing` (upstream's) | — | kept |
| .cargo/mutants.toml excludes (fxdax-gets, shuttle test modules, import's fixtures) | Measurement program; test code cargo-mutants doesn't see | kept |

## Known failures

| List | Entries | Verdict |
|---|---|---|
| scripts/pjdfstest-fuse-known | none | — (fails once a listed failure passes) |
| guest/sim/known-failures | none | — |
| crates/fxfs-posix/known-panics | none | — |
| guest pjdfstest `FXFS_SKIPPED` | none | — |
| xfstests `KNOWN_FAILURES` (18): no unwritten extents (009, 092, 094, 255, 568, 796, 797); reflink extent heuristic (301, 302); EFBIG past a DAX mount's s_maxbytes (303, 525); FIEMAP shared extents (372, 414, 578, 702: TASK-143, open); d_type of specials (401: TASK-144, open); no parent pointers (467, 477: TASK-145, open) | each under a comment saying why | kept; **fixed**: a listed failure that runs and passes now fails the case (but 301 and 302, which pass or not), so the list can't keep a test it no longer needs. **Filed** TASK-149.20.1: xfstests is in no suite |

## Guest cases and checks

| What | Why | Verdict |
|---|---|---|
| 20 cases tagged Manual only: xfstests ×2, linux-checkout ×4, pgbench profile/locks/nofsync ×4, runner-deadlock, bench-* ×9 | Measurements, hours-long, or failing on purpose (`Tag::Manual`) | kept but xfstests: **filed** TASK-149.20.1 |
| `ShouldRun::No`: no Firecracker/kernel/daemon named; pjdfstest without its pin; fsx-faults off the debug kernel (it runs in soak-sanitized); fs-bench without iterations; runner-deadlock unless asked; stats without fxdaxctl | Each says why | kept |
| fxfs-guest-checks: `Dax`/`LargeDax`/`SmallDax` checks | Of a window | kept |
| fxfs-guest-checks: `PageCache` alone: large_folios, scattered_writeback, partial_writeback, writeback_out_of_memory ("Files on DAX are in no page cache") | Stale premise: copying on write, a DAX mount's new files are in the page cache till read in again | **filed** TASK-149.20.3 (needs GCE) |
| `test_short_copy` (the task's example) | Runs in every suite (`All`) | — |
| `if dax` branches in checks (files.rs, seals.rs, follow) | Add DAX's own steps; skip none | — |

## Switches

| Switch | What it turns off | Verdict |
|---|---|---|
| `FXDAX_WORKSPACE_TESTS=0`, `FXDAX_GUEST_TESTS=0` | Which halves a GCE run runs; a run's choice | kept |
| `FXDAX_NO_UNSHARE=1` (GCE hosts) | The runner's buildah unshare isolation, as buildah's re-execution fails there | kept, **filed** TASK-149.20.6 |
| `FXDAX_CHECKS`, `FXDAX_CHECK`, `FXDAX_XFSTESTS`, `SIM_OFF` | Select what runs, for debugging | kept |

## Mutants with no killer (guest/mutants, `Killer: none`)

| Mutant | Why | Verdict |
|---|---|---|
| task-79.49-fault-ignores-fatal-signals, task-79.28-range-too-small, task-150.9-resize-moves-the-last-block, task-153.1-fault-limit, task-164.4-truncate-unwritten | Equivalent, each says how | kept |
| patch-0012-invalidate-unmaps-after | Said no why in its header (guest/sim/README.md had it: equivalent copying on write) | **fixed**: its header says why |
| task-44.19-map-to-unlocked | Out of every oracle's reach: a region's put unmapping less than the host maps | kept, **filed** TASK-149.20.4 (the oracle) |
| fxdax, fxfs-posix, fxfs-sim-diff, guest/sim mutants | Each names its killer (their --check fails on none) | — |
