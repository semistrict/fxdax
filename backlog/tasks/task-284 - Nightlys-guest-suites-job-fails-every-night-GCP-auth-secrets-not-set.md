---
id: TASK-284
title: >-
  Nightly's guest suites on GitHub's own KVM runners, no GCP: no GCE host, no
  GCS bucket, no GCP secrets
status: To Do
assignee: []
created_date: '2026-11-09 07:26'
updated_date: '2026-11-11 07:28'
labels:
  - ci
dependencies: []
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scheduled Nightly fails guest-suites at google-github-actions/auth, because the GCP secrets are not set: Nightly 42971082927 at aaba26d2b, and every night since at least 142195ae2. So the nightly guest suites have not run from CI: followers, snapshots, volumes, pjdfstest, and xfstests generic/quick with and without DAX.

The user, 2026-11-11: do not use GCP. Nightly's guest-suites job therefore runs on GitHub-hosted runners, which have /dev/kvm (TASK-330.1's sandboxes run VMs there), through scripts/test-e2e.sh or scripts/lib/test-linux-host.sh. It runs as CI's guest jobs do: Firecracker in its jailer under its real seccomp filter, and branches kept in a local OSI-licensed S3 server (RustFS, as TASK-330.1 sets it up) instead of a GCS bucket. Results go to workflow artifacts. If the suite outgrows one runner's time limit or 16 GB, split it across a matrix. Remove google-github-actions/auth, setup-gcloud, the id-token permission and the FXDAX_GCE_*/bucket variables from nightly.yml, and update ci.yml's comments that say VMs need GCE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 nightly.yml uses no GCP: no google-github-actions, no gcloud, no GCE/GCS variables or secrets
- [ ] #2 A dispatched Nightly runs the whole nightly suite (runner list --tag nightly) on GitHub's runners and passes, known failures checked as before
- [ ] #3 ci.yml's and nightly.yml's comments describe where VMs actually run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
