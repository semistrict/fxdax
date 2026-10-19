# extent_remote's mutants, triaged (TASK-44.1, TASK-44.5)

The sweep (sweep/, GCE n2-standard-8, before the host's 7 h limit) tried 569 of 611 mutants: 417
caught, 60 missed, 17 timeouts, 75 unviable; the 42 left untried (sweep/untried.txt) were all
writer.rs's. The missed and the untried, by their names after the tests below (rerun.txt, 134 with
cargo-mutants' field deletions, which -F does not narrow), ran again on two hosts (rerun-er0/,
rerun-er1/): 104 caught, 28 missed, 2 timeouts. rerun-er1 began before the writer tests came; those
writer.rs mutants were killed since, by cargo-mutants run locally over the functions.

## Killed by tests

- fetch.rs: a read ending with a part fetches none after it; long tables read whole; short objects
  whose trailers lie; tails kept per object and forgotten past TAILS; read-ahead within a part,
  elsewhere, and over STREAMS objects.
- layout.rs: tails read once or twice; lying trailers; data ranges, empty, reversed, past the end;
  a branch's extents under its prefix.
- reader.rs: checkpoints and segments where their keys say (a checkpoint of another branch is
  corrupt now).
- collector.rs (its first unit tests): orphaned extents, extents ahead of their segments,
  compaction once, moves outliving their objects, lost branches failing a pass unless deleted.
- store.rs: uploads by kind; the probe failing a bucket without conditional writes; `now`, which
  the collector's grace needs.
- writer.rs: fenced by a later writer's segment; stopped when taken over; minting past what
  commits seal; another's object at an extent's key; checkpoints written once; `location`.
- bin/fxdax-gc.rs: each pass's holder is random now, not time and pid mixed.

## Code they showed redundant, gone

- fetch.rs:352 `parts.flown += 1`: flights' numbers; a flight goes only once its last reader has.
- writer.rs's `Created::Exists(object) if *object.bytes == *bytes`: `bucket::create` already
  takes its own bytes, found, as created.

## Equivalent

- cache.rs:109:64 `<` as `<=` in Cache::put: free space exactly at the floor plus a part.
- cache.rs:183:43 `|` as `^` in punch: FALLOC_FL_PUNCH_HOLE and FALLOC_FL_KEEP_SIZE share no bit.
- cache.rs:196-209, macOS's punch: compiled out on Linux; killed on macOS by the cache tests (a
  local run over cache.rs: every one of them caught).
- collector.rs:213:53 `<` as `<=` in Pass::compact: a float exactly at the threshold.
- fetch.rs `last + 1` as `last * 1` in read_ahead's start: a part read ahead that a demand read
  is fetching joins its flight.
- fetch.rs `>` as `>=` in fetch_tail: 17n + 40 is never 4096.
- reader.rs:63 `last_segment` in State::forked: writers and readers start a fork's chain from
  the same forked state; any start would do.
- store.rs GetOptions' `head`: a HEAD that fetches the body it drops, cost, not result.
- writer.rs:152 and the upload's `>` as `>=` on epochs: no two takings share one.
- writer.rs Writer::branch: in log messages only.

## Caught by timeouts

The 17 (and the reruns' 2): mutants that make the simulation or a recovery loop forever.
