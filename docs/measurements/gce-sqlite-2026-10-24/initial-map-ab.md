# Initial writable-region mapping A/B

Fxfs-DAX, three iterations per side, on the same host with the DAX entry-order fix. The existing 16-page first mapping is the baseline; the candidates were not adopted.

| Workload | Unit | 16-pages | 4-pages | 4-pages vs 16-pages | Ranges overlap |
|---|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 793.9 (790.4–819.1) | 783.2 (756.9–785.3) | 1.01× | no |
| sqlite-pread-scan-warm | MiB/s | 2,649 (2,622–2,691) | 2,623 (2,609–2,711) | 1.01× | yes |
| sqlite-mmap-scan | MiB/s | 3,332 (3,214–3,376) | 3,296 (3,236–3,475) | 1.01× | yes |
| sqlite-mmap-scan-warm | MiB/s | 3,612 (3,556–3,695) | 3,672 (3,593–3,686) | 0.98× | yes |
| sqlite-delete-full-batch1 | txn/s | 253.5 (252.3–264.5) | 252.6 (245.8–255.3) | 1.00× | yes |
| sqlite-delete-full-batch100 | txn/s | 43.21 (42.82–43.51) | 41.64 (40.1–41.76) | 1.04× | no |
| sqlite-wal-full-batch1 | txn/s | 589 (556.3–615.6) | 515.5 (475.8–531.2) | 1.14× | no |
| sqlite-wal-full-batch1-checkpoint | ms | 19.83 (15.2–20.27) | 19.85 (19.79–20.15) | 1.00× | yes |
| sqlite-wal-full-batch100 | txn/s | 94.36 (93.71–95.9) | 90.9 (87.87–92.8) | 1.04× | no |
| sqlite-wal-full-batch100-checkpoint | ms | 111.1 (95.02–143) | 107.4 (102.3–107.6) | 0.97× | yes |
| sqlite-wal-normal-batch1 | txn/s | 1.216e+04 (1.198e+04–1.269e+04) | 1.08e+04 (8,703–1.13e+04) | 1.13× | no |
| sqlite-wal-normal-batch1-checkpoint | ms | 33.23 (16.94–38.42) | 36.56 (32.96–50.4) | 1.10× | yes |
| sqlite-wal-normal-batch100 | txn/s | 170.3 (169.5–171.5) | 168.1 (168–169.5) | 1.01× | no |
| sqlite-wal-normal-batch100-checkpoint | ms | 160.7 (98.2–221.4) | 248.9 (129.3–301.2) | 1.55× | yes |

| Workload | Unit | 16-pages | 64-pages | 64-pages vs 16-pages | Ranges overlap |
|---|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 793.9 (790.4–819.1) | 787.9 (730.2–841.1) | 1.01× | yes |
| sqlite-pread-scan-warm | MiB/s | 2,649 (2,622–2,691) | 2,667 (2,527–2,671) | 0.99× | yes |
| sqlite-mmap-scan | MiB/s | 3,332 (3,214–3,376) | 3,270 (3,253–3,332) | 1.02× | yes |
| sqlite-mmap-scan-warm | MiB/s | 3,612 (3,556–3,695) | 3,647 (3,646–3,673) | 0.99× | yes |
| sqlite-delete-full-batch1 | txn/s | 253.5 (252.3–264.5) | 219.4 (210.9–222.3) | 1.16× | no |
| sqlite-delete-full-batch100 | txn/s | 43.21 (42.82–43.51) | 45.8 (44.89–46.43) | 0.94× | no |
| sqlite-wal-full-batch1 | txn/s | 589 (556.3–615.6) | 533.5 (518.5–542.8) | 1.10× | no |
| sqlite-wal-full-batch1-checkpoint | ms | 19.83 (15.2–20.27) | 36.06 (20.03–39.3) | 1.82× | yes |
| sqlite-wal-full-batch100 | txn/s | 94.36 (93.71–95.9) | 100.4 (97.96–102.2) | 0.94× | no |
| sqlite-wal-full-batch100-checkpoint | ms | 111.1 (95.02–143) | 113.9 (106–123.8) | 1.02× | yes |
| sqlite-wal-normal-batch1 | txn/s | 1.216e+04 (1.198e+04–1.269e+04) | 1.226e+04 (1.205e+04–1.253e+04) | 0.99× | yes |
| sqlite-wal-normal-batch1-checkpoint | ms | 33.23 (16.94–38.42) | 30.54 (18.41–37.91) | 0.92× | yes |
| sqlite-wal-normal-batch100 | txn/s | 170.3 (169.5–171.5) | 174.6 (169.4–186.1) | 0.98× | yes |
| sqlite-wal-normal-batch100-checkpoint | ms | 160.7 (98.2–221.4) | 153.7 (121.6–211.4) | 0.96× | yes |
