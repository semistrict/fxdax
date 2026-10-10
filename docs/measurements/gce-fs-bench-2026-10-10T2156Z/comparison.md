Host: linux. Iterations: fxdax-nodax 3, virtiofs-passthrough 3.

| Workload | Unit | fxdax-nodax | virtiofs-passthrough |
|---|---|---|---|
| create | ops/s | 2,894 (2,253–2,909) | 2,023 (1,948–2,041) |
| stat-cold | ops/s | 5,788 (5,602–5,975) | 13,939 (13,836–13,964) |
| readdir | ops/s | 325,849 (311,653–331,286) | 430,066 (380,067–434,196) |
| unlink | ops/s | 4,031 (3,866–4,220) | 5,136 (4,450–5,395) |
| untar | s | 0.17 (0.16–0.19) | 0.17 (0.17–0.18) |
| make | s | 6.93 (6.92–7.20) | 7.28 (7.16–7.50) |
| build-total | s | 7.10 (7.09–7.39) | 7.45 (7.33–7.67) |

- fxdax-nodax: mtime-ahead-of-guest-ms 0.09 median (0.08–0.10)
- virtiofs-passthrough: mtime-ahead-of-guest-ms -0.21 median (-0.91–-0.20)

