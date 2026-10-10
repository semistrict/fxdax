Host: linux. Iterations: fxdax-nodax 1, virtiofs-passthrough 1.

| Workload | Unit | fxdax-nodax | virtiofs-passthrough |
|---|---|---|---|
| create | ops/s | 1,701 (1,701–1,701) | 1,420 (1,420–1,420) |
| stat-cold | ops/s | 5,181 (5,181–5,181) | 9,869 (9,869–9,869) |
| readdir | ops/s | 346,662 (346,662–346,662) | 358,262 (358,262–358,262) |
| unlink | ops/s | 3,105 (3,105–3,105) | 4,075 (4,075–4,075) |

- fxdax-nodax: mtime-ahead-of-guest-ms 0.10 median (0.10–0.10)
- virtiofs-passthrough: mtime-ahead-of-guest-ms -0.23 median (-0.23–-0.23)

