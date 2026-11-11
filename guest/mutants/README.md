# guest/mutants: past bugs of the guest's Fxfs, put back

Each `*.patch` takes a fix away from the guest's code (`guest/linux/fs/fxfs`, the kernel's
`fs/dax.c` as `guest/linux/patches` leave it, a patch of that series, left out of
`scripts/build-guest-kernel.sh`'s, or `guest/sim`'s model of Fxfs), so that a run with it in shows
whether the test that should find the bug still does. One set, whether the simulator or a
real kernel finds the bug (TASK-160.32). Picked by hand from the code's history, so not a fair sample
of its bugs.

## The header

A patch begins with comments, which `patch` and `git apply` pass over, and two of them say how its
mutant is run and what its fate is:

```
# Kernel: sim
# Killer: scenarios cow_swap_during_write
```

- **`Kernel:`** what it runs on: `sim`, `guest/sim`'s simulator, in seconds; `model`, guest/sim's
  model of Fxfs alone, beside Fxfs, as `crates/fxfs-sim-diff` runs it, for a drift of the model's
  that dax.c in the simulator can't see; or the guest kernel a test host builds for it, `release`,
  `debug` (lockdep and KCSAN) or `kasan`, in minutes on ubu1. Only what the simulator can't find
  needs a real kernel.
- **`Killer:`** the run that must fail with the mutant in. The mutant is killed only if it does:
  another test failing doesn't count, and the run's other tests aren't run.
  - Of the simulator's, a test of `guest/sim/tests` and its arguments: one of one run
    (`hole_mapped_then_written`), a scenario (`scenarios NAME`), a test that takes seeds
    (`interleave`, `failures`), which runs seed after seed until one fails, or the sweep of a run of
    one (`sweep scenarios NAME`, `guest/sim/sweep.sh`), which runs it once for each of its calls
    that may fail, that call failing, until one run fails.
  - Of the model's, `fxfs-sim-diff` and a test of the crate's (`the_models_past_drifts_from_fxfs`).
  - Of a real kernel's, a guest case (`runner list`), and, if it is a checks suite, the check of it
    that fails, as the case's `failed_checks` say (`scripts/guest_results.py`): the case is run
    alone, of its checks that one alone. A case with no check named is killed by its failing at
    all, as a kernel's complaint in its console fails it.
  - `none`, if nothing can kill it, and the comments after say why: equivalent, or out of every
    oracle's reach. It isn't run.

The comments after say what the bug was, how it was found, and how its killer fails.

## Running them

- `guest/sim/mutants.sh` runs those whose kernel is `sim` (`guest/sim/linux.sh guest/sim/mutants.sh`
  elsewhere than on Linux); `--check` applies and reverses each, with no fuzz, as CI's `guest-sim`
  job does.
- `crates/fxfs-sim-diff/mutants.sh` runs every one that changes the model, `Kernel: model` or not,
  against the model's comparison with Fxfs, and fails if one of `Kernel: model` lives; `--check`
  applies and reverses each.
- `scripts/guest-mutants-host.py RESULTS` runs those of a real kernel on a Linux host of one's own
  (`FXDAX_HOST`, ubu1 by default), each a run of `scripts/test-host.sh` with its patch applied;
  `--check` checks every header, and that each of these and the model's applies (`scripts/test.sh`'s
  scripts part).
- `scripts/dax-gate.sh` runs the simulator's first, then the rest, on ubu1: each must be killed.
