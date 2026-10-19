---
id: TASK-50.6
title: Docker and containerd end to end on a guest Fxfs root
status: To Do
assignee: []
created_date: '2026-10-19 20:10'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: medium
ordinal: 155000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandboxes run Docker and containerd inside their VMs. Nothing shows them working on an Fxfs root: overlayfs over Fxfs, their metadata stores, image pulls and container runs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest test boots on an Fxfs root, pulls an image and runs a container with Docker, and with containerd
- [ ] #2 What fails is filed as a task, or documented as unsupported with what programs see
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
