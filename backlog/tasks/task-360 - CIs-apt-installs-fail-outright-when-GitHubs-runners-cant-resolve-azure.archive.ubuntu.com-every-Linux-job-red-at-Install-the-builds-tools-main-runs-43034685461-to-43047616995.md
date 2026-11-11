---
id: TASK-360
title: >-
  CI's apt installs fail outright when GitHub's runners can't resolve
  azure.archive.ubuntu.com: every Linux job red at 'Install the build's tools'
  (main runs 43034685461 to 43047616995)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 15:32'
updated_date: '2026-11-11 16:41'
labels:
  - ci
dependencies: []
priority: medium
type: bug
ordinal: 745000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
2026-11-11, from 15:00 UTC: main's CI runs 43034685461 (7ad3c5236), 43037954231, 43041159143 and 43047616995 (cf757b22e) failed every Linux job at 'Install the build's tools' (.github/workflows/ci.yml): apt-get update warned 'Temporary failure resolving azure.archive.ubuntu.com' and apt-get install then failed (E: Failed to fetch ... exit 100), before any of fxdax's code built. An outage of GitHub's runners' DNS or Azure's mirror, not fxdax's; but one failed fetch fails the whole run, and the runs went red for hours. Options: apt's own retries (-o Acquire::Retries=5) and a retry of the step; or the packages from a cache (actions/cache of /var/cache/apt/archives, or a prebuilt image).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A transient mirror failure retries rather than failing the job
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
One composite action, .github/actions/apt-install, used by every workflow's and composite action's apt step. It drops the image's mirrors whose names don't resolve, appends kernel.org's and MIT's Ubuntu mirrors, and fails the update on any index not fetched. Proved by a CI run and a Sandboxes run (nested in guest-host) during the outage.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
What the image already does (probe run 43052221835, ubuntu-24.04):
- ubuntu.sources fetches through mirror+file:/etc/apt/apt-mirrors.txt: azure.archive (1), archive.ubuntu.com (2), security.ubuntu.com (3).
- zz-retries sets Acquire::Retries 1 and http(s) Timeout 15.
- Azure's resolver 168.63.129.16 timed out on all three ubuntu.com names (10 s each with getent) but resolved mirrors.edge.kernel.org and packages.microsoft.com, so retries and the image's own fallbacks couldn't help.

Fix:
- Kernel.org's and MIT's mirrors, which carry noble, -updates, -backports and -security a few hours fresh, are appended at priority 10 and 11. apt verifies them against the Ubuntu archive key.
- Mirrors whose names don't resolve are dropped. Appending alone left a job in its install step past 13 minutes (run 43053208153), since apt's mirror method tries each mirror for every file with 20 s lookups.
- No name resolving, or an image no longer using the mirror file, fails the step with ::error::. Update runs with APT::Update::Error-Mode=any, so an index no mirror serves fails the update. A healthy run's update prints no warnings (run 42949975927).

Proof:
- CI 43055722986 on t360-apt, during the outage: every Linux job's install passed (clippy scripts' took 52 s, everything from kernel.org).
- Every job passed except test (clippy scripts), on TASK-361's two tests, which already fail on main. guest-sim was still running at landing.
- Sandboxes 43055727871: guest-host's nested apt-install passed in 52 s, then the kernel built.
- scripts/tests/test_ci.py AptInstall drives apt-install.sh with fake getent, sudo and apt-get. It covers all resolving, ubuntu.com unresolved (dropped, warned), none resolving (fails before apt), no mirror file (fails), update failing (install not run), and no apt-get anywhere in .github but the action.
<!-- SECTION:NOTES:END -->
