Host: linux. Iterations: fxdax-nodax 1, virtiofs-passthrough 1.

| Workload | Unit | fxdax-nodax | virtiofs-passthrough |
|---|---|---|---|
| create | ops/s | 1,423 (1,423–1,423) | 1,454 (1,454–1,454) |
| stat-cold | ops/s | 5,173 (5,173–5,173) | 9,590 (9,590–9,590) |
| readdir | ops/s | 324,304 (324,304–324,304) | 349,859 (349,859–349,859) |
| unlink | ops/s | 3,400 (3,400–3,400) | 3,951 (3,951–3,951) |

- fxdax-nodax: mtime-ahead-of-guest-ms 0.15 median (0.15–0.15)
- virtiofs-passthrough: mtime-ahead-of-guest-ms -0.20 median (-0.20–-0.20)

