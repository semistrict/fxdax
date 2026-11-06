# Cutting every replay at every barrier in the model test's every-cut mode

**Tried:** 2026-11-06, TASK-210, not committed: `Setting::second_cuts` gave `every_cut_of_a_short_sequence`
`Crashes::new(seed, 1)`, a cut of the replay at each of its barriers, where it had none.
**Idea:** The every-cut mode never cut a replay, so a mount's free that came before the journal recorded
what its replay discarded (TASK-210) was reached only by the random hunt's cuts of replays, once in a
305 s hunt.
**Result:** With the bug put back, FXDAX_MODEL_EVERY_CUT=300 ran 20,186 cuts at 1,101 barriers and
82,904 cuts of replays in 1,489 s, and passed: it didn't find it. The default 6 cases went from 2.6 s
to 15.7 s (different seeds), about ten times the cost per case.
**Why it's dead:** The rare part isn't the second cut, which a replay's few barriers reach often enough
at random: it's the first cut finding an extent a compaction recorded dying with its allocation's
transaction discarded. Short sequences rarely make that, cut however often. The class is tested where
it's decided instead: `ExtentTable::settle_replay` for each death a mount finds, and a deterministic
every-cut-of-the-mount test (`write_many.rs`).
**Reopen if:** A generator makes a mount's discards common: say, sequences that write while compacting
and then cut with the data lost but the journal kept.
