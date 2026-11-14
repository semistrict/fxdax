# Parts fetched from peers before the bucket (TASK-406.2)

TASK-406.2 has a host's fetchers ask the host that owns a part, by rendezvous hashing of the
object's key and the part's number over the hosts sharing the bucket, before the bucket. Here what
that saves, counted on two extent daemons sharing one bucket.

**Run.** ubu1 (x86_64, 16 cores), 2026-11-14, under `fxjob`, at the TASK-406 branch (bf4317fd and
its assertion fix): `cargo nextest run -p fxdax -E "binary(peers)"`, `crates/fxdax/tests/peers.rs`.
Two `fxdax-extentd` processes, a file bucket, each daemon its own part cache, peers on the
loopback. The first host writes 16 extents of 1 MiB of incompressible data, syncs, and makes a
point of them; the second takes the point, forks it, and reads every extent once. Counted from
each daemon's stats file (`object-store:` and `peers:` lines), once without peers and once with.

| | Second host's ranged GETs | Parts the first gave it | First host's ranged GETs |
|---|---:|---:|---:|
| Alone | 21 | 0 | 7 |
| With peers | 11 | 10 | 7 |

The second host fetched from the bucket only the parts it owns (and what both runs share: its
tails and its own records' reads); the 10 parts the first owns came from the first host's part
cache, where its uploads left the segments, with no GET of the first host's more than it made
alone. Every byte read was checked: the test compares each extent with what was written.

With N hosts reading one base cold, each part is fetched from the bucket once instead of N times,
and each host fetches about 1/N of the parts itself (`remote::peers::tests::
hosts_asking_their_peers_fetch_each_part_once`: three hosts, 8 parts, 16 GETs fewer).
