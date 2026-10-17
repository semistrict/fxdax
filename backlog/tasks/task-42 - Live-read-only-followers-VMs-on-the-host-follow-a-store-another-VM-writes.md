---
id: TASK-42
title: 'Live read-only followers: VMs on the host follow a store another VM writes'
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:18'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
priority: high
ordinal: 120000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Other VMs on the writer's host mount a store read-only while a VM has it mounted read-write, and follow it live: they see each commit as the writer makes it, a prefix of its history, without remounting, and the writer never waits for them. DAX is the primary path. The design, agreed with the user on 2026-10-17, and above all its testing (concurrency bugs here are silent stale reads, so testing comes first): backlog doc-8. Point-in-time mounts fall out of it (a follower that never advances).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A follower VM mounts a store another VM on the host writes, read-only, and sees each of its commits without remounting, shown by the guest torture test (doc-8 testing 6) passing for hours on GCE, DAX on and off
- [ ] #2 Every layer of doc-8's testing exists and catches the bugs it is for: each has a mutation check (the rule it guards turned off makes it fail)
- [ ] #3 The writer's commit latency and pgbench tps are unchanged with followers attached (doc-8 testing 7)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
