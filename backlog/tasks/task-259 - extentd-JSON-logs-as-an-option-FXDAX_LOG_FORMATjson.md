---
id: TASK-259
title: 'extentd: JSON logs as an option (FXDAX_LOG_FORMAT=json)'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 14:04'
updated_date: '2026-11-08 14:40'
labels:
  - logging
  - extentd
dependencies: []
priority: medium
ordinal: 691000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd logs plain text lines to stderr (TASK-107): fine for a person, but a host's log pipeline (journald → a collector, an OpenTelemetry agent) has to parse them with regexes, and key-values with spaces in their values cannot be split back out. An option that writes one JSON object per record lets the pipeline read every field as is. Text stays the default. fxdaxctl does not log (it prints its results), so the option is the daemon's, through the fxdax logger any binary installing it shares.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 With FXDAX_LOG_FORMAT=json, extentd writes one JSON object per line per record: an RFC 3339 UTC timestamp, the level, the target, the message, each key-value as its own field, and request_id when the record is about a guest request
- [ ] #2 FXDAX_LOG_FORMAT unset or text keeps the text lines; any other value refuses to start, naming the variable
- [ ] #3 A unit test covers the JSON encoding: escaping, key-values by type, the level, request_id
- [ ] #4 A test shows the option switches the format
- [ ] #5 The daemon doc comment (fxdax-extentd.rs) documents the option
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. logging.rs: Settings from FXDAX_LOG_FORMAT (text|json) and FXDAX_LOG_LEVEL (default, TARGET=LEVEL). 2. JSON encoder: timestamp RFC 3339 UTC (humantime), level, target, message, request_id/store, key-values typed; reserved names kv_-prefixed. 3. One write per line. 4. Tests: encoding, format switch, levels. 5. Document in fxdax-extentd.rs.
<!-- SECTION:PLAN:END -->
