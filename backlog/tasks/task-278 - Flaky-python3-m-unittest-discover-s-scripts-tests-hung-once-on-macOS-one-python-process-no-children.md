---
id: TASK-278
title: >-
  Flaky: python3 -m unittest discover -s scripts/tests hung once on macOS, one
  python process, no children
status: To Do
assignee: []
created_date: '2026-11-09 05:45'
updated_date: '2026-11-11 17:29'
labels:
  - flaky
dependencies: []
priority: low
type: bug
ordinal: 710000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
During TASK-276 (2026-11-09) a full run of the scripts tests (257 tests, normally ~40 s) sat for more than 10 minutes in one python3 process with no child processes and no output past the dots; killed, the next run passed in 40 s. Which test hung is unknown: the run was not verbose. Candidates are those with sockets or threads (a ResourceWarning names an unclosed 127.0.0.1 socket in a thread, test_linux_host Network fetch tests).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The hanging test is found (a verbose loop of runs) and fixed, or a per-test timeout makes a hang fail with its name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11, again (TASK-361's work, macOS):
- `python3 -m unittest discover -s scripts/tests 2>&1 | sed -n '$p'`, not verbose, ran past 120 s in the foreground and then 30 minutes in the background before it was killed. Which test hung is unknown: no output got past the pipe, and its processes weren't listed before the kill.
- Six runs right after (three with -v to a file under timeout --foreground, one -v through a pipe, one -v in a background shell, one under timeout) passed 278 tests in 39-40 s each.

A mechanism found meanwhile, for hangs in agents' shells on this Mac: ~/.local/bin/grep, head and tail are symlinks to eish (github.com/semistrict/eish), which comes before /usr/bin on agents' PATH.
- When its stdin is a pipe, eish feeds the real tool through an io.TeeReader (entry.go: cmd.Stdin = stdin), not the file itself. Go's exec.Cmd.Wait waits for that copy to reach EOF. So `grep PATTERN FILE`, which never reads stdin, doesn't return until the stdin it inherited closes: forever under a pipe kept open.
- Measured: `sleep 8 | { grep -c localhost /etc/hosts; }` returns after 8.0 s with eish's grep and 0.0 s with /usr/bin/grep; eish's head the same.
- That's how TASK-149.45's agent saw scripts/check-tla.sh hang in a background shell: some of its greps now read < /dev/null (lines 159-180), but those at 123-140 don't.
- The scripts tests themselves mostly run scripts with PATH=<fakes>:/usr/bin:/bin. Those that keep the inherited PATH (test_firecracker_forward, test_firecracker_cargo) run scripts with no grep, head or tail, and none found in the suite calls eish. TASK-278's own instance (no children) wasn't eish either.
- The fix belongs in eish: hand the child an os.Pipe's read end and don't wait for the copy once the child exits. Not in fxdax's scripts.
<!-- SECTION:NOTES:END -->
