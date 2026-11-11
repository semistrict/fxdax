---
id: TASK-347
title: >-
  xfstests' image fetches xfstests-dev from git.kernel.org with no retry nor
  mirror: a 502 fails a nightly part
status: To Do
assignee: []
created_date: '2026-11-11 08:43'
labels:
  - ci
  - bug
dependencies: []
ordinal: 742000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Nightly run 42981086953 (TASK-284, GitHub runner), part 4: xfstests-fxfs-guest-dax's root image build failed at its Containerfile's 'git -C /xfstests fetch --depth 1 https://git.kernel.org/pub/scm/fs/xfs/xfstests-dev.git refs/tags/v2026.10.15': 'RPC failed; HTTP 502'. The case then SKIPs (podman build failed), which the runner counts as a failure (0/0 passed), so a transient outage of one host fails the nightly. TASK-297 gave the bench images a host cache of downloads (sha256-checked, curl retried, several sources); xfstests' source isn't among them. Fetch it the same way (a tarball by its tag, checked, with a mirror such as GitHub's kdave/xfstests), or retry the fetch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 xfstests' image takes its source through the host's download cache, sha256-checked, retried, from more than one source
- [ ] #2 With git.kernel.org unreachable, xfstests-fxfs-guest builds its image and runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
