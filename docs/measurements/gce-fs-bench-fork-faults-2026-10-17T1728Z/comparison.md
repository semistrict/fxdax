Host: linux. VMM: libkrun. Iterations: fxfs-guest-dax-fork-cold 3, fxfs-guest-dax-fork-warm 3.

| Workload | Unit | fxfs-guest-dax-fork-cold | fxfs-guest-dax-fork-warm | fxfs-guest-dax-fork-warm vs fxfs-guest-dax-fork-cold |
|---|---|---|---|---|
| base-copy-usr | s | 2.74 (2.74–2.74) | 2.78 (2.78–2.78) | 1.02× |
| base-pgbench-init | s | 11.5 (11.5–11.5) | 11.6 (11.6–11.6) | 1.00× |
| fork-mount | s | 0.19 (0.19–0.19) | 0.19 (0.19–0.19) | 0.99× |
| fork-read-usr | s | 4.66 (4.58–4.66) | 0.57 (0.57–0.57) | 0.12× |
| fork-pg-start | s | 0.25 (0.24–0.25) | 0.14 (0.14–0.14) | 0.59× |
| fork-pgbench-select | tps | 22,763 (22,559–40,248) | 48,147 (47,995–48,478) | 0.47× |

- fxfs-guest-dax-fork-cold: extentd-fault-ns 12,194,856,363 median (12,187,234,213–12,353,651,573)
- fxfs-guest-dax-fork-cold: extentd-faults 4,697 median (4,660–4,845)
- fxfs-guest-dax-fork-cold: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-cold: extentd-map-ns 9,628,853 median (8,857,054–9,958,867)
- fxfs-guest-dax-fork-cold: extentd-maps 379 median (379–379)
- fxfs-guest-dax-fork-cold: extentd-page-in-bytes 1,393,197,056 median (1,389,002,752–1,403,682,816)
- fxfs-guest-dax-fork-cold: extentd-page-ins 1,354 median (1,350–1,364)
- fxfs-guest-dax-fork-cold: extentd-read-bytes 4,165,632 median (4,165,632–4,165,632)
- fxfs-guest-dax-fork-cold: extentd-read-stored-bytes 4,165,632 median (4,165,632–4,165,632)
- fxfs-guest-dax-fork-cold: extentd-reads 597 median (597–597)
- fxfs-guest-dax-fork-cold: extentd-unpacks 1,954 median (1,936–1,959)
- fxfs-guest-dax-fork-cold: guest-commit-ms 3.59 median (3.45–4.01)
- fxfs-guest-dax-fork-cold: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-cold: guest-free-ms 621 median (590–634)
- fxfs-guest-dax-fork-cold: guest-free-requests 2,157 median (2,157–2,157)
- fxfs-guest-dax-fork-cold: guest-info-ms 0.42 median (0.34–0.44)
- fxfs-guest-dax-fork-cold: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-cold: guest-map-ms 240 median (218–253)
- fxfs-guest-dax-fork-cold: guest-map-requests 378 median (378–378)
- fxfs-guest-dax-fork-cold: guest-read-ms 148 median (131–164)
- fxfs-guest-dax-fork-cold: guest-read-requests 597 median (597–597)
- fxfs-guest-dax-fork-cold: guest-root-ms 0.59 median (0.46–0.59)
- fxfs-guest-dax-fork-cold: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-cold: guest-unmap-ms 5.46 median (5.29–5.47)
- fxfs-guest-dax-fork-cold: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-cold: guest-write-ms 16.9 median (16.2–22.1)
- fxfs-guest-dax-fork-cold: guest-write-requests 49.0 median (49.0–50.0)
- fxfs-guest-dax-fork-warm: extentd-fault-ns 875,937 median (818,830–957,359)
- fxfs-guest-dax-fork-warm: extentd-faults 117 median (117–117)
- fxfs-guest-dax-fork-warm: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-map-ns 43,483,141 median (41,812,607–45,195,234)
- fxfs-guest-dax-fork-warm: extentd-maps 378 median (378–378)
- fxfs-guest-dax-fork-warm: extentd-page-in-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-page-ins 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-read-bytes 4,165,632 median (4,165,632–4,165,632)
- fxfs-guest-dax-fork-warm: extentd-read-stored-bytes 4,165,632 median (4,165,632–4,165,632)
- fxfs-guest-dax-fork-warm: extentd-reads 596 median (596–596)
- fxfs-guest-dax-fork-warm: extentd-unpacks 15.0 median (15.0–15.0)
- fxfs-guest-dax-fork-warm: guest-commit-ms 3.77 median (3.65–3.97)
- fxfs-guest-dax-fork-warm: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-warm: guest-free-ms 489 median (458–692)
- fxfs-guest-dax-fork-warm: guest-free-requests 2,167 median (2,167–2,167)
- fxfs-guest-dax-fork-warm: guest-info-ms 0.42 median (0.40–0.48)
- fxfs-guest-dax-fork-warm: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-warm: guest-map-ms 260 median (258–265)
- fxfs-guest-dax-fork-warm: guest-map-requests 377 median (377–377)
- fxfs-guest-dax-fork-warm: guest-read-ms 121 median (118–123)
- fxfs-guest-dax-fork-warm: guest-read-requests 596 median (596–596)
- fxfs-guest-dax-fork-warm: guest-root-ms 0.59 median (0.58–0.67)
- fxfs-guest-dax-fork-warm: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-warm: guest-unmap-ms 4.97 median (4.83–5.18)
- fxfs-guest-dax-fork-warm: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-warm: guest-write-ms 14.3 median (13.4–14.8)
- fxfs-guest-dax-fork-warm: guest-write-requests 50.0 median (50.0–50.0)

Every workload is within 2× of ext4.
