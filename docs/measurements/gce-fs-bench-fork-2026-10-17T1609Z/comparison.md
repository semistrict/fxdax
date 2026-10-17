Host: linux. VMM: libkrun. Iterations: fxfs-guest-dax-fork-cold 3, fxfs-guest-dax-fork-warm 3.

| Workload | Unit | fxfs-guest-dax-fork-cold | fxfs-guest-dax-fork-warm | fxfs-guest-dax-fork-warm vs fxfs-guest-dax-fork-cold |
|---|---|---|---|---|
| base-copy-usr | s | 2.79 (2.79–2.79) | 2.95 (2.95–2.95) | 1.06× |
| base-pgbench-init | s | 11.5 (11.5–11.5) | 11.6 (11.6–11.6) | 1.00× |
| fork-mount | s | 0.20 (0.20–0.20) | 0.19 (0.18–0.19) | 0.95× |
| fork-read-usr | s | 6.16 (6.15–6.16) | 0.57 (0.56–0.57) | 0.09× |
| fork-pg-start | s | 0.25 (0.25–0.25) | 0.15 (0.14–0.15) | 0.60× |
| fork-pgbench-select | tps | 20,611 (20,414–36,620) | 46,683 (26,441–47,564) | 0.44× |

- fxfs-guest-dax-fork-cold: extentd-fault-ns 13,895,993,635 median (13,857,465,508–14,066,763,854)
- fxfs-guest-dax-fork-cold: extentd-faults 37,888 median (37,815–37,961)
- fxfs-guest-dax-fork-cold: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-cold: extentd-map-ns 8,469,835 median (8,225,480–8,991,487)
- fxfs-guest-dax-fork-cold: extentd-maps 379 median (379–380)
- fxfs-guest-dax-fork-cold: extentd-page-in-bytes 1,269,121,024 median (1,268,465,664–1,269,329,920)
- fxfs-guest-dax-fork-cold: extentd-page-ins 9,724 median (9,719–9,725)
- fxfs-guest-dax-fork-cold: extentd-read-bytes 4,161,536 median (4,161,536–4,161,536)
- fxfs-guest-dax-fork-cold: extentd-read-stored-bytes 4,161,536 median (4,161,536–4,161,536)
- fxfs-guest-dax-fork-cold: extentd-reads 595 median (595–595)
- fxfs-guest-dax-fork-cold: guest-commit-ms 3.64 median (3.50–3.79)
- fxfs-guest-dax-fork-cold: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-cold: guest-free-ms 554 median (543–671)
- fxfs-guest-dax-fork-cold: guest-free-requests 2,164 median (2,164–2,164)
- fxfs-guest-dax-fork-cold: guest-info-ms 0.42 median (0.39–0.50)
- fxfs-guest-dax-fork-cold: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-cold: guest-map-ms 114 median (105–127)
- fxfs-guest-dax-fork-cold: guest-map-requests 378 median (378–379)
- fxfs-guest-dax-fork-cold: guest-read-ms 138 median (132–140)
- fxfs-guest-dax-fork-cold: guest-read-requests 595 median (595–595)
- fxfs-guest-dax-fork-cold: guest-root-ms 0.53 median (0.52–0.61)
- fxfs-guest-dax-fork-cold: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-cold: guest-unmap-ms 5.39 median (5.17–5.68)
- fxfs-guest-dax-fork-cold: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-cold: guest-write-ms 16.7 median (16.6–17.1)
- fxfs-guest-dax-fork-cold: guest-write-requests 51.0 median (50.0–51.0)
- fxfs-guest-dax-fork-warm: extentd-fault-ns 724,676 median (643,406–749,049)
- fxfs-guest-dax-fork-warm: extentd-faults 117 median (117–117)
- fxfs-guest-dax-fork-warm: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-map-ns 40,126,992 median (39,541,557–40,206,796)
- fxfs-guest-dax-fork-warm: extentd-maps 381 median (381–381)
- fxfs-guest-dax-fork-warm: extentd-page-in-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-page-ins 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-read-bytes 4,194,304 median (4,194,304–4,194,304)
- fxfs-guest-dax-fork-warm: extentd-read-stored-bytes 4,194,304 median (4,194,304–4,194,304)
- fxfs-guest-dax-fork-warm: extentd-reads 604 median (604–604)
- fxfs-guest-dax-fork-warm: guest-commit-ms 3.40 median (3.28–3.56)
- fxfs-guest-dax-fork-warm: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-warm: guest-free-ms 588 median (509–645)
- fxfs-guest-dax-fork-warm: guest-free-requests 2,150 median (2,150–2,150)
- fxfs-guest-dax-fork-warm: guest-info-ms 0.44 median (0.38–0.48)
- fxfs-guest-dax-fork-warm: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-warm: guest-map-ms 259 median (254–270)
- fxfs-guest-dax-fork-warm: guest-map-requests 380 median (380–380)
- fxfs-guest-dax-fork-warm: guest-read-ms 123 median (112–124)
- fxfs-guest-dax-fork-warm: guest-read-requests 604 median (604–604)
- fxfs-guest-dax-fork-warm: guest-root-ms 0.71 median (0.54–0.73)
- fxfs-guest-dax-fork-warm: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-warm: guest-unmap-ms 5.29 median (4.58–5.31)
- fxfs-guest-dax-fork-warm: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-warm: guest-write-ms 13.4 median (12.6–14.4)
- fxfs-guest-dax-fork-warm: guest-write-requests 49.0 median (49.0–50.0)

Every workload is within 2× of ext4.
