# Working on fxdax

## Batch Rust changes; compile and test once per batch

Write a large batch of changes before compiling or testing anything: several features or
fixes, days of work, each still its own commit. Then compile once, fix what the compiler finds,
build the guest kernel once if the batch touches it, and run the tests once, on GCE
(`scripts/test-gce.sh`; Linux tests run there, not in Lima).

A batch is several backlog tasks worked through one after another, in one worktree, by one agent.
It does not mean tasks run in parallel: subagents or worktrees side by side each pay for their own
compiles and test runs, which a batch exists to pay once. Pick the batch's tasks, read them, write
all their changes, and only then compile.

Do not alternate "write one change, compile, test, write the next". A compile of this workspace,
a guest kernel build and a GCE round each take minutes; paying them per change wastes most of
the time. Rust's compiler catches most of what a per-change build would, all at once at the end
of a batch.

- Read the code a change touches, and its tests, before writing it, so that the batch compiles
  with few fixes.
- Write the tests for each change with it, in the same batch.
- Then: `cargo build` (or `cargo check`) once, `cargo clippy`, the macOS suite
  (`scripts/test.sh`), and one GCE run of the workspace and the guest cases.
- While a batch's GCE run goes, write the next batch rather than wait for it.
- Measure a change on its own only when isolating it is the point of the measurement (an A/B on
  one host).

## Commit in batches, only what compiled and passed its tests

Commit a batch once it has compiled and its unit tests have run and passed (the workspace's
`cargo nextest`, and the GCE run for what only runs on Linux): never a change that has not been
compiled and tested. A batch may still be several commits, one logical change each, made together
after the batch's test run.

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
