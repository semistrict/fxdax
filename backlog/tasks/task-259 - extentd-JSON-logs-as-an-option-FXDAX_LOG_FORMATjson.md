---
id: TASK-259
title: 'extentd: JSON logs as an option (FXDAX_LOG_FORMAT=json)'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 14:04'
updated_date: '2026-11-08 16:23'
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
- [x] #1 With FXDAX_LOG_FORMAT=json, extentd writes one JSON object per line per record: an RFC 3339 UTC timestamp, the level, the target, the message, each key-value as its own field, and request_id when the record is about a guest request
- [x] #2 FXDAX_LOG_FORMAT unset or text keeps the text lines; any other value refuses to start, naming the variable
- [x] #3 A unit test covers the JSON encoding: escaping, key-values by type, the level, request_id
- [x] #4 A test shows the option switches the format
- [x] #5 The daemon doc comment (fxdax-extentd.rs) documents the option
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. logging.rs: Settings from FXDAX_LOG_FORMAT (text|json) and FXDAX_LOG_LEVEL (default, TARGET=LEVEL). 2. JSON encoder: timestamp RFC 3339 UTC (humantime), level, target, message, request_id/store, key-values typed; reserved names kv_-prefixed. 3. One write per line. 4. Tests: encoding, format switch, levels. 5. Document in fxdax-extentd.rs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: logging::tests a_json_record_has_a_field_for_each_of_its_parts (time, level, target, message, request_id, store, typed key-values, escaping of quotes, newlines, tabs and control bytes, a key-value named as a field renamed kv_level; parsed back with serde_json), the_format_is_the_variables (unset and text give the text line, json the JSON object, yaml refused naming FXDAX_LOG_FORMAT, FXDAX_LOG_LEVEL default and per-target levels, a bad level refused), the two text tests kept, a_text_record_names_its_request. Also FXDAX_LOG_LEVEL (default and TARGET=LEVEL, longest target deciding), needed to see extent_remote::store debug records. fxdaxctl does not log (prints results), so nothing there; fxdax-mount and the VMM tests share the logger. Each record one write of the line. Main CI 42497804619 green at 76bbd8de8; Linux Docker 801/805 (4 container-only), macOS 561/561.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd logs JSON with FXDAX_LOG_FORMAT=json: one object a line of RFC 3339 UTC timestamp, level, target, message, request_id and store when the record is about a guest request, and each key-value as a typed field; text stays the default, any other value refuses to start; FXDAX_LOG_LEVEL sets levels by target. Documented in fxdax-extentd.rs. Verified by the encoding and format-switch tests and main CI 42497804619 green at 76bbd8de8.
<!-- SECTION:FINAL_SUMMARY:END -->
