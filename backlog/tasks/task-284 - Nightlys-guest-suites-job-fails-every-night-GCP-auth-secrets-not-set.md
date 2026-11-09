---
id: TASK-284
title: 'Nightly''s guest-suites job fails every night: GCP auth secrets not set'
status: To Do
assignee: []
created_date: '2026-11-09 07:26'
labels:
  - ci
dependencies: []
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scheduled Nightly (latest 42572359042 at 142195ae2; also 79f7ae639, bd0934d31, f81174090) fails guest-suites at google-github-actions/auth: 'must specify exactly one of workload_identity_provider or credentials_json'. nightly.yml needs the repository secrets GCP_WORKLOAD_IDENTITY_PROVIDER and GCP_SERVICE_ACCOUNT and the variables FXDAX_GCE_PROJECT and FXDAX_NIGHTLY_BUCKET; gh secret list shows none set. So the nightly GCE guest suites (followers, snapshots, volumes, pjdfstest, xfstests generic/quick) have not run from CI. Needs the repository owner to create the workload identity pool/provider and service account and set them; until then the job should say it is skipped rather than fail.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The secrets and variables are set and a dispatched Nightly runs guest-suites
- [ ] #2 Without them the job is skipped with a notice, not failed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
