---
id: TASK-219
title: A published Go client of extentd's control API
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 20:09'
updated_date: '2026-11-06 21:09'
labels: []
dependencies: []
priority: medium
ordinal: 655000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team calls extentd's gRPC control API (fxdax.extentd.v1: Grant, soon GetSnapshot) from Go. They generate their own bindings from crates/fxdax/proto/fxdax/extentd/v1/extentd.proto with protoc-gen-go and protoc-gen-go-grpc, keep ~3,900 generated lines in their repo, regenerate by hand at each pin bump, and can drift silently from the daemon they run. fxdax publishes the generated code itself, versioned with its commits, so that `go get <module>@<fxdax commit>` is the API of the extentd built from that commit. A thin wrapper like the Rust extentd::control::Client is wanted but optional.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A Go module in the repo (go/), whose package extentdv1 holds extentd.pb.go and extentd_grpc.pb.go generated from extentd.proto, which names it in option go_package
- [x] #2 Generation is reproducible, its tools pinned, and CI fails when the generated files differ from what the proto generates
- [x] #3 `go get github.com/semistrict/fxdax/go@<commit>` gives the API of that commit's extentd; how is documented
- [x] #4 A thin client: dialing the control socket, its health service, and calls' errnos as Go errors (errors.Is(err, syscall.ENOENT)); tested in Go, and against the real daemon on Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. go/ a module (github.com/semistrict/fxdax/go), go_package in extentd.proto naming its package extentd/v1 (extentdv1).
2. go/generate.sh: buf (pinned, go run) compiles the .proto, protoc-gen-go and protoc-gen-go-grpc as go.mod tools; --check generates into a scratch dir and diffs.
3. client.go: NewClient(socket) dials the Unix socket, with the health client, and an interceptor that has errors carry the fxdax-errno trailer as syscall.Errno (errors.Is/As), the status kept (GRPCStatus).
4. Tests: an in-process fake server (errno, none, garbled); daemon_test.go (tag extentd) against fxdax-extentd itself on Linux.
5. scripts/test.sh part go, CI matrix part go; docs (integrator guide, README); decision in OPEN_QUESTIONS.md.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
CI run 42292199272: test (go) passed on Linux, with TestDaemon against fxdax-extentd; guest-sim jobs still running when pushed at the user's word. Drift check exits 1 on a stale or stray .pb.go (checked locally). go list -m github.com/semistrict/fxdax/go@56ccc78e3 resolves to v0.0.0-20261106205349-56ccc78e3a98.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
go/ module github.com/semistrict/fxdax/go: extentdv1 generated from extentd.proto (go_package) by go/generate.sh (buf pinned, generators as go.mod tools), a CI drift check, NewClient with errno-carrying errors. Landed on main at 56ccc78e3.
<!-- SECTION:FINAL_SUMMARY:END -->
