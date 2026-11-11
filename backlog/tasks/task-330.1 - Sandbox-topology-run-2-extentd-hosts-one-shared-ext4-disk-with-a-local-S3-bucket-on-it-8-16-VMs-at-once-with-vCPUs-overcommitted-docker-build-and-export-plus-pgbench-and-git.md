---
id: TASK-330.1
title: >-
  Sandbox topology run: 2 extentd hosts, one shared ext4 disk with a local S3
  bucket on it, 8-16 VMs at once with vCPUs overcommitted, docker build and
  export plus pgbench and git
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 14:00'
updated_date: '2026-11-11 10:32'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 730000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Builds on TASK-310's fxfs-guest-shared-disk case and TASK-311's fxfs-guest-docker-build-dax. Use an OSI-licensed S3 server (RustFS is Apache-2.0; check the licence before adding one). The run fails on any guest kernel report (TASK-311's first-oops capture), any VM let go, any slow framing over 1 s, or a sandbox command over its budget. Short mode under 10 min for every main push; long mode nightly.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The run exists, passes on main, and fails on 6a513509a..96c7808b8's write-behind regression put back (TASK-310) as a mutant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Guest case fxfs-guest-sandboxes: two extentd hosts (tokens 1, 2) sharing one S3 bucket (RustFS) whose data the case checks is on its own ext4 disk; each host imports the sandboxes' image (Alpine+PostgreSQL+git, Docker static+buildx+BusyBox archive) once and boots each VM on a fork (Fxfs root); roles in turn, each host each role: builder (8 vCPUs, DAX root, fxfs-guest-docker-build-dax's guest), pgbench (fxfs-guest-pgbench's guest), git (new), writer (fxfs-guest-shared-disk's guest); DAX windows on every other round's own stores; vCPUs at once > host CPUs checked; each guest times echo/uname/cat every second.
2. Checks: kernel reports (the runner's consoles check), each VM ending well, 'letting a VM process go', any framing > 1000 ms, any command > 1000 ms, the run's stores writing > 1.5x the guests' bytes; budgets.py over both daemons.
3. scripts/sandboxes.sh [short|long]: RustFS 1.0.4 (Apache-2.0) in a container, its data on FXDAX_SANDBOX_DISK (ext4), the bucket made, test-e2e.sh fxfs-guest-sandboxes; prints the case's summary.
4. .github/workflows/sandboxes.yml (reusable, dispatchable): KVM via udev on GitHub's runner, guest kernel cached by scripts/guest-kernel-key.sh, vmstat and RSS logged. Not yet in ci.yml (main fails the framing check: TASK-310, TASK-336) nor nightly.yml (long mode doesn't fit a 16 GB runner: TASK-330.5).
5. Mutant crates/fxdax/mutants/framed-in-copies.patch, Killer: scripts/sandboxes.sh short; crates/fxdax/mutants.sh runs script killers.
6. Verified on ubu1 and GitHub's runner; macOS and ubu1 suites; CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10 (claude): built (branch t330-land): guest case fxfs-guest-sandboxes, scripts/sandboxes.sh [short|long], .github/workflows/sandboxes.yml (CI short on every push, nightly long), crates/fxdax/mutants/framed-in-copies.patch (Killer: scripts/sandboxes.sh short; mutants.sh runs script killers), scripts/guest-kernel-key.sh.
Topology: two extent daemons (host-a, host-b, tokens 1 and 2), one S3 bucket (RustFS 1.0.4, Apache-2.0, checked: gh api repos/rustfs/rustfs/license) whose data the case checks is on its own ext4 disk (st_dev), each host importing the sandboxes' image (Alpine 3.22 + PostgreSQL + git, Docker 29.9.1 static + buildx 0.38.2 + BusyBox archive) once (1.7-2.0 s) and booting each VM on a fork, its root Fxfs. Roles in turn, each host each role: builder (8 vCPUs, DAX root, fxfs-guest-docker-build-dax's guest: buildx build, --load, save, tar export), pgbench (fxfs-guest-pgbench's guest: load, run, check, crash recovery, pg_checksums), git (init, commits, gc, clone --no-local, fsck --strict), writer (fxfs-guest-shared-disk's guest, LangSmith's 768 MiB fsynced + 3,000 x 8 KiB); DAX windows on every other round's own stores. Short: 8 VMs at once, 28 vCPUs (8 on 8 CPUs under fxjob --bench, 7x on a 4-CPU runner). Long: 16 at once, 32 in all, smaller VMs (10.75 GiB at once).
Checks: kernel reports (the runner's console check, first_report), each VM ending well and saying its role's OK, 'letting a VM process go' in either daemon's log, any framing past 1000 ms, any sandbox command (echo, uname, cat on the root, every second in each guest) past 1000 ms, and a host's stores writing past 1.3 times their guests' bytes (TASK-330.2's budget; added: see below); budgets.py over both daemons as for every run.
Why the writes check: the slowest framing alone doesn't tell the regression from main on ubu1's shared disk (ext4 on the rpool zvol, nvme shared with other agents). Interleaved A/B, fxjob --bench, short mode:
- builders 256 MiB (fxfs-guest-docker-build-dax's default): main round 1 pass, slowest framing 429/250 ms, stores 1.18/1.19x the guests' bytes (4.98 GiB a host), segments framed in place 56/58; round 2 FAIL on framings, slowest 3,403/5,323 ms (sync 0.8-2.9 s, dirs 0.5-4.4 s, write <= 0.2 s), stores 1.19/1.18x, durable lag max 4.6 s. Mutant: round 1 FAIL framings 1,404/1,555 ms and writes 1.83/1.83x (framed in place 0); round 2 FAIL framings 3,017/3,016 ms and writes 1.84/1.83x.
- builders 512 MiB, before the writes check: main slowest 1,456/1,481 ms (fail) then 628/957 ms (pass); mutant 955/980 ms (pass) then 1,080/770 ms (fail). Sum of slow framings: main 13-15 s, mutant 27-33 s. Framed bytes: main 0.35 GB, mutant 5.9 GB of 7.26 GB.
- Commands: slowest 9-77 ms everywhere, main and mutant (budget 1 s). Daemons' memory 600-730 MiB of 2.5-3.1 GiB budgets. VMs ran 72-100 s; a short run 63-200 s on ubu1.
So: the mutant is killed every round, by the writes check always and by framings in 3 of 4; main passes the writes check every round, and the framing check on ubu1 passes or fails with the disk's queue: TASK-310's remainder (framings' file and directory syncs behind a saturated disk). AC #1's pass on main waits on TASK-310.
Found and filed: TASK-335 (vm::complaints took x86's CPU-vulnerability advisory 'Speculative Return Stack Overflow: WARNING: See ...' for a kernel report: every guest case failed on GitHub's AMD runners; fixed in this batch, test red with the bug back). Harness fixes on the way: test-e2e.sh fetches Docker and buildx for the sandboxes too; pgbench's load role makes its data directory on a store of its own; FXDAX_PGBENCH_SCALE.
CI: GitHub's ubuntu-24.04 runners have nested KVM (AMD EPYC 9V74, 4 vCPUs, 16 GB, one ext4 root disk mounted nobarrier); probe run 42905951685: guest kernel built in 12 min, artifacts 10 min, fxfs-guest and fxfs-guest-docker-build-dax guests ran (docker build round 16 s) and failed only on TASK-335.

GitHub runner, short mode on main (probe branch t330-probe, the sandboxes workflow alone): FAIL on framings only: slowest 1,159/1,292 ms, 2 framings a host past 1 s, two of them write phase alone (1,078 and 1,039 ms: CPU, 28 vCPUs on 4), the rest mostly sync; stores 1.20/1.22x the guests' 4.25 GiB a host; commands slowest 166 ms; budgets.py ok (anon 653-689 MiB); VMs 165 s, the case 203 s, the job ~32 min with the kernel built (15 min) and artifacts (12 min), less with them cached. Filed TASK-336 (framings' write phase starved of CPU on an overcommitted host).
So main fails the exact framing check on both hosts, ubu1's (syncs, TASK-310) and GitHub's (syncs and write phases, TASK-310 and TASK-336). ci.yml doesn't call the short mode yet, as every push would be red; .github/workflows/sandboxes.yml runs it by hand (workflow_dispatch, once on main) and the nightly runs long. Hooking it to every push is a ci.yml job of three lines (uses: ./.github/workflows/sandboxes.yml, mode: short), once TASK-310 and TASK-336 let main pass it.
Linux suite on ubu1 (clippy, workspace): 2482 of 2484; the two failures environmental and passing alone: extentd::tests::a_store_after_a_workers_write_waits_for_one_lift hung 56 min while fxdax.slice was past its 16 GiB MemoryHigh (filed TASK-337: the worker's store waited in handle_userfault while the fault server was throttled; alone 7.7 s); durable_commits_keep_the_open_segments_pages_cached_on_the_host, whose store is in CARGO_TARGET_TMPDIR, on ZFS in my target (TASK-309's), passes with it on ext4.

AC #1's mutant, through crates/fxdax/mutants.sh framed-in-copies on ubu1 (fxjob --bench, the tree built with the patch by the script killer, test-e2e.sh's guest and firecracker parts first): killed twice, 338 s ('writes: 1.83/1.85x past 1.3'; framings under a second that round, 693/720 ms) and 367 s ('framings: host-a framed 4 segments slower than 1000 ms, the slowest 2097.6 ms'; writes 1.91/1.91x). With the interleaved A/B rounds above: killed 6 of 6 runs, by the writes check every time.
Long mode on GitHub's runner: the runner went down ('received a shutdown signal') 7-7.5 min into the run twice, at 10.75 GiB of guest memory at once and again at 9 GiB (16 VMs, 56 vCPUs on 4 CPUs). Being reproduced on ubu1 with 4 CPUs and a 15 GB cap to see what runs out; until it runs whole there, the nightly job fails at that step.
The repository's Actions caches are at 10.6 GB, past GitHub's 10 GB: the kernel's entry (40 MB) was evicted within half an hour once; the workflow keeps it, and no cargo cache of its own.

Sizing for GitHub's 16 GB runner, found by the long mode taking it down (vmstat and RSS now in the job's log): VMs builder 1.5 GiB / pgbench 1 / git 0.5 / writer 0.75 (7.5 GiB at once), daemons 3 GiB each (4 GiB: caches climbed to 2.9 GB each and the runner ran out; 2 GiB: windows refused, ENOMEM pinning their private pages), DAX windows root 1 GiB / own stores 512 MiB. Long mode: 8 at once of 32 (LangSmith's controlled shape), more work each. Writes check now over the run (both hosts) at 1.5 (a host's own ratio follows its roles' mix: 1.39 for one left with pgbench's copies).
Final sizes, verified: ubu1 short main x2: round 1 failed one framing (1,233 ms) and nothing else, round 2 passed every check; writes 1.16x both; no VM failed. Mutant through mutants.sh: killed in 346 s, 'framings: host-b ... 1343.3 ms' and 'writes: 1.80 times, past 1.5'. GitHub short (run 42944951179): every VM well, commands <= 382 ms, writes 1.19x, one framing 1,211 ms: fails as main does. GitHub long: the runner lost 66 min in, no log (TASK-330.5); being run whole on ubu1.
Other flakes met in this task's CI runs, filed: TASK-339 (macOS: forks_read_what_they_inherited_compacted_twice) and TASK-341 (Linux: extent_store::crash openings_cut_at_each_write_open_again, an opening at epoch 22 holding epoch 21's contents); each passed on rerun. Final CI on the code (run 42951885432) green.

Rebased onto main dfd9eb484 (its TASK-276.6 DAX changes, guest dax.c included): a fresh kernel (FXDAX_GUEST_KERNEL_JOBS=4) and artifacts on ubu1, short mode PASSED every check: slowest framing 368/325 ms, stores 1.19x, commands <= 41 ms, daemons' anon 712/722 MiB, VMs 85.5 s. CI on the rebased code (run 42979578798) green after reruns of TASK-341's flake, main's (reproduced on ubu1 with main's extent_store, 1 in 3 runs). Landed on main. Left In Progress: AC #1's pass on main is not steady, the framing check flapping past 1 s on ubu1's disk and on GitHub's runner (TASK-310, TASK-336); the short mode goes in ci.yml and the long in nightly.yml once they pass (TASK-330.5 for the long's size).
<!-- SECTION:NOTES:END -->
