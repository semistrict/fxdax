# Working on fxdax

## Batch Rust changes; compile and test once per batch

Write a large batch of changes before compiling or testing anything: several features or
fixes, days of work, each still its own commit. Then compile once, fix what the compiler finds,
build the guest kernel once if the batch touches it, and run the tests once, on ubu1
(`scripts/test-host.sh`; Linux tests run there, not in Lima).

A batch is several backlog tasks worked through one after another, in one worktree, by one agent.
It does not mean tasks run in parallel: subagents or worktrees side by side each pay for their own
compiles and test runs, which a batch exists to pay once. Pick the batch's tasks, read them, write
all their changes, and only then compile.

Do not alternate "write one change, compile, test, write the next". A compile of this workspace,
a guest kernel build and a host run each take minutes; paying them per change wastes most of
the time. Rust's compiler catches most of what a per-change build would, all at once at the end
of a batch.

- Read the code a change touches, and its tests, before writing it, so that the batch compiles
  with few fixes.
- Write the tests for each change with it, in the same batch.
- Then: `cargo build` (or `cargo check`) once, `cargo clippy`, the macOS suite
  (`scripts/test.sh`), and one ubu1 run of the workspace and the guest cases.
- While a batch's ubu1 run goes, write the next batch rather than wait for it.
- Measure a change on its own only when isolating it is the point of the measurement (an A/B on
  one host).

### Linux runs go to ubu1, never GCP

The user's rule (2026-11-11): no GCP in any form, CI included. Every Linux run goes to ubu1, a
physical host others' agents share, through the `scripts/*-host.sh` drivers
(`scripts/lib/host.sh`; `FXDAX_HOST` names another host): `test-host.sh`, `bench-host.sh`,
`bench-ab-host.sh`, `perf-gate-host.sh`, `soak-host.sh`, `soak-workloads-host.sh`,
`mutants-host.sh`, `guest-mutants-host.py`, `gets-host.sh`, `migrate-host.sh`,
`fork-time-host.sh`. A run is a worktree there of a commit origin has (push first; HEAD of a clean
tree by default, `FXDAX_HOST_COMMIT` names another), its own target directories, its job under
`fxjob` (`--bench` for measurements), results copied back, worktree removed. Guest kernels come
from a cache on the host keyed by their sources (`host_kernel`). A driver stopped mid-run leaves
the job running: `DRIVER resume RUN RESULTS` fetches it, `DRIVER cancel RUN` ends it. Pass the run
number as `FXDAX_HOST_RUN` (e.g. `r1040`). ubu1's setup is its `~/docs/system-setup.d` steps: what a
run needs installed goes there as a step, never into a run.

## Commit in batches, only what compiled and passed its tests

Commit a batch once it has compiled and its unit tests have run and passed (the workspace's
`cargo nextest`, and the ubu1 run for what only runs on Linux): never a change that has not been
compiled and tested. A batch may still be several commits, one logical change each, made together
after the batch's test run.

## Work lands on main: no long-lived branches

A branch lives only as long as the batch on it: once its tests pass, it goes into main, is pushed, and is deleted, locally and on the remote. Pushing to main needs no question. Never leave finished work waiting on a branch, nor start a batch from a branch other than main: merge main in often, so that nothing drifts far from it.

## Don't wait on the user: decide, and record it in OPEN_QUESTIONS.md

Never ask the user anything directly: not in a reply, not with a question tool, not as a choice at the end of a message. The user doesn't read through output for questions. Everything you'd need from them goes in `OPEN_QUESTIONS.md` at the repo root.

A decision doesn't wait for the user. Make your best guess, act on it, and record it under `# Decisions` in `OPEN_QUESTIONS.md`: in a few lines, what you decided, why, what else was possible, and what undoing it would take. The user reads the file at leisure and overturns what they disagree with.

Only what you can't do yourself stays under `# Open questions`: an action only the user can take (signing in, anything the user's own instructions reserve to them, such as deleting a test), or a fact only they know with no reasonable guess. An entry says what's needed, why it matters, what you recommend, and what happens until it's answered; go on with whatever doesn't depend on it.

Check the file before starting work: an answer may be there. As soon as the user answers a question or confirms or overturns a decision, wherever they say it, take its entry out of the file and act on it: the file holds only what the user hasn't seen through yet.

## No migrations, ever: formats change outright

The user's rule (2026-11-05), not a phase: fxdax never migrates data. On-disk, bucket and wire formats change whenever that's the better design, and the new code refuses the old format (`Error::Format` and its kind) rather than reading, converting or upgrading it: no migration code, no compatibility shim, no dual-format reader, nothing kept to read old data, and no task or plan step proposing one. Don't trade a design down to avoid a format change.

## No silent fallbacks to a costlier path: fail hard

The user's rule (2026-11-11): when the cheap way of doing something can't work, fail hard (refuse to start or open, return the error) so the setup gets fixed. Never fall back, silently or with only a log line, to a way that costs vastly more: a copy where a rename failed, the bucket where a local file was meant to serve, a slow path on an unexpected error. Check such a precondition up front where it can be checked (two directories on one filesystem, a cache usable), and treat an existing warn-and-carry-on path that degrades performance as a bug. Found by TASK-350: the host cache's rename of uploaded WAL segments failed across filesystems (EXDEV), a warning hid it, and every read past the daemon's memory went to the bucket, ~30 ms each.

## Never write a test around a bug

The user's rule (2026-11-12): when a test only passes by steering clear of what the code under test
gets wrong (inputs picked so the bug can't show, a bound loosened to match it, a check left out),
that is a bug found. File it as a task first, and name the task in a comment at the workaround. A
test bent around a bug with no task merges never. The workaround goes when the bug is fixed. Found
by TASK-293: the collector never compacted compressed segments, as it measured live bytes against
their compressed size. TASK-175's compaction test had fed it incompressible bytes "so that the
objects' sizes are their extents'" a week before, with no task filed.

## Every bug exposes a blind spot: close it, then look around it

A bug or crash, however it was found and whoever caused it, shows code the tests don't exercise
or don't check. The fix is half the work. For each one:

1. **Name the blind spot:** why the tests stayed green. The path never ran; it ran and nothing
   checked the result; a fake differs from the real thing; an assertion is off in that build; the
   failure needs two things no test combines.
2. **Add the test for the class, not the instance:** extend a generator, an oracle, a fake or an
   invariant, so that it finds this bug and its kin. Put the bug back and see the test go red. A
   seed pinned as a regression test is on top of this, not instead of it.
3. **Look around it:** find the same blind spot elsewhere. Search for code sharing the pattern, and
   use coverage (`FXDAX_GUEST_KERNEL_BUILD=coverage` and `scripts/guest-coverage.py` for `fs/fxfs`)
   to list what near the bug no test runs. Each place found gets a test or a task.
4. **Record it** in the fix's task notes: how it was found, the blind spot, the test added, and what
   step 3 turned up. Testing too large for the fix goes in a task under TASK-149, ranked by how
   likely it is to find bugs.

It is done when those notes exist and the test went red with the bug back.

## Every bug questions the structure: could this class be made impossible?

Before fixing a bug, think hard about the structure of the code around it, at every level up to
the architecture. Could the code have been shaped so that this whole class of bug can't happen: a
type that can't hold the bad state, one owner of what two places keep in step, an interface with no
wrong order to call it in? Is anything about how it's structured suboptimal, even at a very high
level? Look across past bugs too: `backlog search` and `dead-ends/` show whether this one is a
repeat.

- **A small structural problem:** refactor first, in a commit of its own, then fix the bug on top.
- **A fundamental or architectural reason, above all one a pattern of bugs points to:** bring it to
  the user's attention, with the bugs it explains and what you'd change. Don't restructure on your
  own, and don't let the fix hide it: fix the bug as it stands if that can't wait.

Say what you concluded in the fix's task notes, even if the structure is fine.

## Dead ends: check them before you try, add to them when you stop

`dead-ends/` holds every approach that was tried and given up on, a file each, kept for good, so
that nothing is tried again blind.

- Before designing or trying an approach, read `dead-ends/` for it (`rg` the directory for the
  subsystem and the idea). If it is there, say so, and go on only with what is different this time.
- When you give up on an approach, or a measurement shows it isn't worth landing, add its file in
  the same change that records the decision, whether or not any code of it survives.
- A wrong theory that cost real time counts, as does an experiment that worked but can't land.
- Keep the files true: if a dead end's "Reopen if" comes about and it then works, update its file
  to say so rather than delete it. Don't delete files.

Each file is `dead-ends/<short-kebab-name>.md`, short, with these parts:

```markdown
# <the approach, in a line>

**Tried:** date, tasks, branches or commits it lives on.
**Idea:** why it looked right.
**Result:** what happened, with the numbers and the runs that show it.
**Why it's dead:** the reason, not the symptom.
**Reopen if:** what would have to change.
```

This codebase is yours: what it's for and how it's built are your call, and you'll keep building on it.
