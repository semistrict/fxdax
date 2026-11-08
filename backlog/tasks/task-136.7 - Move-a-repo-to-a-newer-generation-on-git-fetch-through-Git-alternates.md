---
id: TASK-136.7
title: 'Move a repo to a newer generation on git fetch, through Git alternates'
status: In Progress
assignee: []
created_date: '2026-10-31 04:49'
updated_date: '2026-11-08 05:35'
labels: []
dependencies:
  - TASK-136.2
  - TASK-136.4
  - TASK-136.5
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 474000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A mounted worktree's base doesn't change, but `git fetch` in a sandbox should not download what infrastructure already published. Ramon chose the Git level first: objects are content-addressed, so reaching a newer generation's objects through alternates cannot conflict with anything the sandbox wrote, and needs no Fxfs change.

Git has no pre-fetch hook (from memory, unchecked), so a remote helper installed through `url.insteadOf` runs first. As only the sandbox manager attaches, it asks through the guest agent. An in-place swap of base layers stays possible later: doc-10 records what it would need.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 With a newer generation published, `git fetch` in a sandbox transfers no object that generation holds, shown by a test counting what the remote sent
- [ ] #2 A fast-forward `git pull` after it writes only the changed files into the writable layers, and the next `git status` hashes only those
- [ ] #3 Local commits, staged and unstaged changes and untracked files survive the move, and objects of the earlier generation stay readable
- [ ] #4 A sandbox manager refusing the attach leaves an ordinary fetch, which succeeds
- [ ] #5 The read-only generation and its alternates path keep working after snapshot, restore and reopening a stopped store
- [ ] #6 doc-10 records the writable layers' growth across repeated pulls on a Linux-sized checkout
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Upcall: newest URL -> named NAME | refused WHY (crates/fxfs-automount/src/upcall.rs, a Manager trait answering both asks).
2. git-remote-fxfs (crates/fxfs-automount/src/git.rs): asks newest(URL), looks up ROOT/NAME/.git/objects (the automounter attaches and mounts it), replaces any earlier generation's line in alternates with it, then hands the URL to Git: exec git remote-<scheme> for http(s)/ftp(s); for local paths, the connect capability to git upload-pack/receive-pack. Refusal or failure: ordinary fetch. Unit tests: alternates moving, transports, connect's protocol, a repo moved by a fake manager.
3. Guest test fxfs-guest-git-fetch: publisher publishes repo at A (g0) and B (g1), then upstream at C as a volume; consumer's checkout is g0 (automounted) with a local commit, staged and unstaged changes and an untracked file; fetch through the helper points alternates at g1 and receives exactly rev-list B..C's objects (loose, fetch.unpackLimit), none of g1's; pull --ff-only writes only the changed files (strace), status reads only those and the unstaged one; changes kept, g0's objects read; a URL the manager refuses clones ordinarily, no alternates; snapshot (volumes frozen) and restore, then reboot: the checkout reads through alternates, which automount again. Host checks the asks.
4. AC #6 (writable layers' growth across repeated pulls on a Linux-sized checkout, doc-10): a manual case on GCE after the above lands.
5. doc-10's Moving section; OPEN_QUESTIONS decision (helper's transports).
<!-- SECTION:PLAN:END -->
