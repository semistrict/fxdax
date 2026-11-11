---
id: TASK-284
title: >-
  Nightly's guest suites on GitHub's own KVM runners, no GCP: no GCE host, no
  GCS bucket, no GCP secrets
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 07:26'
updated_date: '2026-11-11 15:15'
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
- [x] #1 nightly.yml uses no GCP: no google-github-actions, no gcloud, no GCE/GCS variables or secrets
- [x] #2 A dispatched Nightly runs the whole nightly suite (runner list --tag nightly) on GitHub's runners and passes, known failures checked as before
- [x] #3 ci.yml's and nightly.yml's comments describe where VMs actually run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Share RustFS's start (scripts/lib/rustfs.sh) between test-rustfs.sh, sandboxes.sh and a new scripts/test-e2e-rustfs.sh (test-e2e.sh's cases, branches in a local S3 bucket).
2. test-e2e.sh: artifacts built once hold the jailer, so prebuilt runs can be jailed; it sources the pjdfstest pin itself.
3. runner list --shard K/N: every Nth case from the Kth, so a matrix runs a suite with no list of names.
4. Composite actions guest-host and guest-kernel, shared by sandboxes.yml and nightly.yml.
5. nightly.yml: guest-build (artifacts + kernel once), guest-suites matrix of parts as root, jailed, seccomp real, RustFS bucket, results as artifacts; no GCP. ci.yml comments.
6. Tests: scripts/tests (test_ci Nightly, containers, bench_ab, pjdfstest pin), guest part on ubu1; dispatched Nightly on the branch, whole suite green.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Run 42981086953: every jailed case with a socket failed EACCES, results in the runner's home (0750) -> work dir /mnt/nightly; test-e2e.sh now fails fast if the jailed user can't reach the results (its own mktemp default, 0700, had the same flaw: now 0755). Run 42989250162: still EACCES for API sockets, which go in TMPDIR (0755, the runner's) -> 1777, and test-e2e.sh checks TMPDIR is the jailed user's to write in (verified on ubu1: 0755 fails the check, 1777 passes). xfstests (both, ~70-90 min a part on GitHub) failed on generic/211 and 679, reached since TASK-149.25's sized scratch and never run since (no nightly): filed TASK-348 (211, SIGBUS overwriting through a mapping on a full copy-on-write Fxfs) and TASK-349 (679, fallocate's unwritten extents, the known class); both listed in KNOWN_FAILURES with reasons. TASK-347 filed: xfstests' image fetch from git.kernel.org 502'd once. Blind spot: jailed runs on hosts other than GCE never ran; the reach checks now fail them fast. ubu1: guest part 122+99+8 passed (shard tests).

Rebased onto main once TASK-330.1 landed. Nightly 43005951347 (7e4b73533) and 43023717997 (rebased, 4f9884108): green, 67 of 67 nightly cases passed in 8 parts, none skipped (pjdfstest and xfstests both ways ran), run.json's knobs: FXDAX_FIRECRACKER_JAIL=1, seccomp real, FXDAX_EXTENTD_REMOTE=s3://fxdax/e2e. Parts' times: 5-10 min but parts 3 and 4 (xfstests, ~1h45-2h15), within the jobs' 330 min. CI 43023721722 on the rebased branch: every job green but the workspace part's extent_store::crash recovers_from_its_bucket_with_and_without_its_disk, TASK-341, which fails main's CI too (43006118431); an earlier branch CI hit TASK-343 (noted there). No workspace crate changed. scripts/upstream-changes.py --check passes. DoD #1 left unchecked: Linux's workspace suite isn't green on main (TASK-341), unrelated to this change.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Nightly's guest suites run on GitHub's runners with no GCP: guest-build builds the artifacts (the jailer now among them) and the kernel once; guest-suites, 8 parts (runner list --shard K/N, every Nth case from the Kth), run test-e2e-rustfs.sh --tag nightly as root, jailed under the real seccomp filter, branches in RustFS on the runner, results as artifacts. RustFS's start is scripts/lib/rustfs.sh's, shared by test-rustfs.sh, test-e2e-rustfs.sh and sandboxes.sh; runner set-up and the kernel cache are composite actions shared with sandboxes.yml; test-e2e.sh sources the pjdfstest pin and fails fast if the jailed user can't reach the results or write TMPDIR. Comments in ci.yml/nightly.yml no longer say VMs need GCE; OPEN_QUESTIONS' GCP-secrets question gone. Verified: dispatched Nightly 43023717997 on the rebased branch green, 67/67 cases; scripts' tests and the guest part (ubu1) pass. Found and filed: TASK-347 (xfstests' git.kernel.org fetch), TASK-348 (generic/211 SIGBUS), TASK-349 (generic/679); 211 and 679 listed as known.
<!-- SECTION:FINAL_SUMMARY:END -->
