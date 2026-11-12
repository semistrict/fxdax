---
id: TASK-353
title: >-
  fxjob leaves a job's root processes running when its scope is stopped or
  OOM-killed (ubu1 system-setup.d/146)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 12:12'
updated_date: '2026-11-12 08:07'
labels:
  - bug
  - testing
dependencies: []
priority: medium
ordinal: 743000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxjob (ubu1:~/docs/system-setup.d/146-fxdax_cgroups.sh) runs a job in a scope of the user's systemd manager; sudo inside it stays in the scope, but the user's manager can't signal root's processes: when the kernel's OOM killer kills one process at the job's MemoryMax, systemd stops the scope with 'Failed to kill control group ... Operation not permitted', and the root processes run on (TASK-344's Linux suite, 2026-11-11: nextest and its test binaries, as root, after the scope failed 'oom-kill'). scripts/lib/host-remote.sh now reaps them itself (reap, by /proc/*/cgroup); other agents' fxjob users don't. Make fxjob itself kill what is left of its scope once systemd-run returns (sudo kill of the scope's cgroup.procs, as ramon may sudo), as a new version of step 146, rolled out so that fxjobs running as it is replaced aren't broken (write the new script beside and mv it into place).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A job whose sudo'd processes outlive its scope's end (OOM or stop) leaves none running after fxjob returns
- [x] #2 Step 146 rewritten and run on ubu1 without breaking fxjobs running then
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxjob: after systemd-run returns, kill what is left of its scope through the scope cgroup's cgroup.kill (sudo -n), waiting until the cgroup is unpopulated (30 s at most, then fail naming the pids); trap HUP/INT/TERM so a cancel's TERM is waited out and reaped too.
2. Step 146: install_file replaces only what changed and reloads/re-execs only for changed units, so re-running it beside running jobs (bench among them) only swaps fxjob (mv of a new file).
3. Prove red/green on ubu1: stop, OOM and TERM-to-group jobs with a sudo'd grandchild, old fxjob vs new.
4. host-remote.sh's reap: keep unless fxjob covers every path.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 146 rewritten on ubu1 (written beside as .146-fxdax_cgroups.sh.new, mv'd into place, run by system-setup.sh at 02:56 EST beside run-r3305's fxjob --bench and others' work): install_file now leaves files whose content and mode match, and reloads/re-execs only for what changed, so the run replaced /usr/local/bin/fxjob alone (mktemp + mv), no daemon-reload or re-exec; r3305's isolation stayed root throughout. Before: ~/scratch/t353/146.before-t353.sh (deleted with the scratch); the step's file is not in git.
fxjob reaps through the scope cgroup's cgroup.kill (sudo -n), which takes descendants and racing forks, then waits until the cgroup is unpopulated (30 s, then fails naming the pids); HUP/INT/TERM trapped with ':' so the command (in the same process group) gets them and fxjob reaps after.
Proof, ubu1, old fxjob (red) then new (green), a job whose sudo'd root bash spawns sleep (sudo-rs kills its direct child when killed, not grandchildren):
- systemctl --user stop of the scope: old exit 137, left root bash + sleep 1353; new exit 137, nothing left.
- OOM at --mem 200M (python3 bytearray 1 GiB; journal: oom-kill, 'Failed to kill control group ... Operation not permitted'): old left root bash + sleep 1354; new nothing.
- TERM to the job's process group (host-remote cancel), grandchild ignoring TERM: old fxjob died at once, sleep 1355 left; new waited, wrote cgroup.kill, nothing left.
- Installed new fxjob after rollout: stop, OOM and an ordinary job that leaves a root process behind all leave nothing; exit statuses kept (137, 143, 0, 7).
host-remote.sh's reap kept (decision): fxjob doesn't cover every path it handles: cancel's TERM kills the job's shell (no trap), freeing the lock while fxjob still reaps; an fxjob killed outright, or a job of an fxjob older than this, leaves its scope to reap there. Its comment says so; test_host.Reap passes.

DoD #1 not run: no Rust changed (fxjob lives in ubu1's step 146; host-remote.sh's change is a comment, test_host.Reap passes).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxjob (ubu1 step 146) kills what is left of its scope once systemd-run returns, root's processes too, through the scope's cgroup.kill under sudo, and waits until it is empty; signals to fxjob are waited out and reaped after. Step 146 now replaces and reloads only what changed, so its run beside running jobs swapped fxjob alone (mv). Proven red/green on ubu1 for a stopped scope, an OOM-killed one and a cancel's TERM, each with a sudo'd grandchild: old fxjob left root processes, new none. host-remote.sh's reap kept for cancel's race and fxjobs killed outright or older.
<!-- SECTION:FINAL_SUMMARY:END -->
