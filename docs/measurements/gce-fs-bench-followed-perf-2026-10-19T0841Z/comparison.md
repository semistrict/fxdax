Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 1, fxfs-guest-dax-followed-idle 1.

| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed-idle | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|
| pgbench-init | s | 4.22 (4.22–4.22) | 7.52 (7.52–7.52) | 1.78× |
| pgbench-tps | tps | 1,550 (1,550–1,550) | 1,136 (1,136–1,136) | 1.37× |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -2.54 median (-2.54–-2.54)
- fxfs-guest-dax-followed-idle: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: mtime-ahead-of-guest-ms -1.93 median (-1.93–-1.93)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 500 | B | $0.000200 |
| get-range | 1,726 | B | $0.000690 |
| head | 0 | B | $0.000000 |
| list | 6 | A | $0.000030 |
| create | 507 | A | $0.002535 |
| update | 4 | A | $0.000020 |
| overwrite | 0 | A | $0.000000 |
| delete | 2 | free | $0.000000 |
| all | 2,745 | | $0.003475 |

Uploaded 80.8 MiB (segments 80.6 MiB, extents 0 B, checkpoints 204.4 KiB, moves 0 B, other 78 B); downloaded 11.5 KiB.

Followers: at most 2 at once, at most 65 commits and 688 ms behind their writer, keeping at most 214.0 MiB of its frees.

Every workload is within 2× of ext4.
