Host: linux. VMM: libkrun. Iterations: fxfs-guest-dax-fork-cold 3, fxfs-guest-dax-fork-warm 3.

| Workload | Unit | fxfs-guest-dax-fork-cold | fxfs-guest-dax-fork-warm | fxfs-guest-dax-fork-warm vs fxfs-guest-dax-fork-cold |
|---|---|---|---|---|
| base-copy-usr | s | 2.73 (2.73–2.73) | 3.05 (3.05–3.05) | 1.12× |
| base-pgbench-init | s | 11.5 (11.5–11.5) | 11.5 (11.5–11.5) | 1.00× |
| fork-mount | s | 0.20 (0.20–0.20) | 0.19 (0.19–0.19) | 0.98× |
| fork-read-usr | s | 5.71 (5.54–5.76) | 0.57 (0.57–0.58) | 0.10× |
| fork-pg-start | s | 0.25 (0.25–0.25) | 0.14 (0.14–0.15) | 0.59× |
| fork-pgbench-select | tps | 21,620 (21,295–37,608) | 26,655 (26,521–26,873) | 0.81× |

- fxfs-guest-dax-fork-cold: extentd-fault-ns 13,225,236,016 median (12,744,256,612–13,515,623,263)
- fxfs-guest-dax-fork-cold: extentd-faults 36,681 median (36,671–36,793)
- fxfs-guest-dax-fork-cold: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-cold: extentd-map-ns 9,668,812 median (9,547,420–10,274,172)
- fxfs-guest-dax-fork-cold: extentd-maps 380 median (380–380)
- fxfs-guest-dax-fork-cold: extentd-page-in-bytes 1,403,912,192 median (1,400,061,952–1,404,256,256)
- fxfs-guest-dax-fork-cold: extentd-page-ins 1,367 median (1,363–1,367)
- fxfs-guest-dax-fork-cold: extentd-read-bytes 4,218,880 median (4,218,880–4,218,880)
- fxfs-guest-dax-fork-cold: extentd-read-stored-bytes 4,218,880 median (4,218,880–4,218,880)
- fxfs-guest-dax-fork-cold: extentd-reads 610 median (610–610)
- fxfs-guest-dax-fork-cold: extentd-unpacks 2,002 median (1,997–2,025)
- fxfs-guest-dax-fork-cold: guest-commit-ms 3.52 median (3.44–3.56)
- fxfs-guest-dax-fork-cold: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-cold: guest-free-ms 485 median (474–560)
- fxfs-guest-dax-fork-cold: guest-free-requests 2,144 median (2,144–2,144)
- fxfs-guest-dax-fork-cold: guest-info-ms 0.41 median (0.40–0.42)
- fxfs-guest-dax-fork-cold: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-cold: guest-map-ms 221 median (190–247)
- fxfs-guest-dax-fork-cold: guest-map-requests 379 median (379–379)
- fxfs-guest-dax-fork-cold: guest-read-ms 144 median (136–153)
- fxfs-guest-dax-fork-cold: guest-read-requests 610 median (610–610)
- fxfs-guest-dax-fork-cold: guest-root-ms 0.53 median (0.51–0.60)
- fxfs-guest-dax-fork-cold: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-cold: guest-unmap-ms 5.62 median (5.60–5.69)
- fxfs-guest-dax-fork-cold: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-cold: guest-write-ms 26.5 median (18.6–26.6)
- fxfs-guest-dax-fork-cold: guest-write-requests 49.0 median (49.0–50.0)
- fxfs-guest-dax-fork-warm: extentd-fault-ns 754,119 median (697,713–794,318)
- fxfs-guest-dax-fork-warm: extentd-faults 117 median (117–117)
- fxfs-guest-dax-fork-warm: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-map-ns 41,535,084 median (41,417,160–43,272,359)
- fxfs-guest-dax-fork-warm: extentd-maps 381 median (381–381)
- fxfs-guest-dax-fork-warm: extentd-page-in-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-page-ins 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-read-bytes 4,218,880 median (4,218,880–4,218,880)
- fxfs-guest-dax-fork-warm: extentd-read-stored-bytes 4,218,880 median (4,218,880–4,218,880)
- fxfs-guest-dax-fork-warm: extentd-reads 610 median (610–610)
- fxfs-guest-dax-fork-warm: extentd-unpacks 13.0 median (13.0–13.0)
- fxfs-guest-dax-fork-warm: guest-commit-ms 3.45 median (3.38–3.49)
- fxfs-guest-dax-fork-warm: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-warm: guest-free-ms 511 median (504–513)
- fxfs-guest-dax-fork-warm: guest-free-requests 2,145 median (2,145–2,145)
- fxfs-guest-dax-fork-warm: guest-info-ms 0.39 median (0.38–0.43)
- fxfs-guest-dax-fork-warm: guest-info-requests 2.00 median (2.00–2.00)
- fxfs-guest-dax-fork-warm: guest-map-ms 267 median (263–273)
- fxfs-guest-dax-fork-warm: guest-map-requests 380 median (380–380)
- fxfs-guest-dax-fork-warm: guest-read-ms 122 median (118–123)
- fxfs-guest-dax-fork-warm: guest-read-requests 610 median (610–610)
- fxfs-guest-dax-fork-warm: guest-root-ms 0.61 median (0.47–0.66)
- fxfs-guest-dax-fork-warm: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-warm: guest-unmap-ms 5.11 median (5.08–5.13)
- fxfs-guest-dax-fork-warm: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-warm: guest-write-ms 13.5 median (12.7–14.4)
- fxfs-guest-dax-fork-warm: guest-write-requests 50.0 median (49.0–50.0)

Every workload is within 2× of ext4.
