| Case | Workload | Unit | base | landed | landed vs base | ext4-guest-kernel landed vs base | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 8.593e+04 (8.274e+04–9.088e+04) | 8.484e+04 (6.725e+04–8.867e+04) | 1.01× | 1.12× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.128e+05 (1.001e+05–1.331e+05) | 1.127e+05 (9.588e+04–1.318e+05) | 1.00× | 1.02× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.046e+05 (9.908e+04–1.235e+05) | 1.178e+05 (9.822e+04–1.275e+05) | 0.89× | 1.03× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.154e+05 (1.081e+05–1.214e+05) | 1.098e+05 (1.055e+05–1.136e+05) | 1.05× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 3,472 (3,200–3,737) | 3,973 (3,631–4,531) | 0.87× | 1.02× | within drift |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 248.8 (238.6–264.2) | 222.2 (207.9–230.4) | 0.89× | 1.02× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 815.1 (544.8–1,630) | 588.8 (350.2–1,417) | 0.72× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 3.184 (2.704–3.376) | 3.024 (2.544–3.312) | 0.95× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 10.3 (9.28–11.46) | 10.56 (6.624–20.1) | 1.02× | 1.10× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 4,197 (3,807–4,357) | 4,116 (3,765–4,452) | 1.02× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 448.5 (436.2–456.7) | 428 (415.7–436.2) | 0.95× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 1,286 (962.6–2,212) | 1,700 (1,073–2,376) | 1.32× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.712 (3.536–4.08) | 3.872 (3.696–4.128) | 1.04× | 0.92× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 39.94 (25.22–46.85) | 31.74 (27.52–51.46) | 0.79× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 4,628 (4,551–4,796) | 5,172 (4,935–5,224) | 0.89× | 1.03× | real |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 505.9 (493.6–518.1) | 489.5 (485.4–497.7) | 0.97× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 5,997 (5,341–6,324) | 4,489 (3,949–4,489) | 0.75× | 1.06× | real |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 3.76 (3.6–3.888) | 3.728 (3.632–3.856) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 49.66 (35.58–60.67) | 64.64 (56.06–78.34) | 1.30× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 4,615 (4,447–4,763) | 5,063 (4,977–5,265) | 0.91× | 0.96× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 565.2 (544.8–593.9) | 577.5 (553–585.7) | 1.02× | 0.95× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 1.73e+04 (1.645e+04–1.901e+04) | 1.52e+04 (1.422e+04–1.671e+04) | 0.88× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 3.68 (3.6–3.824) | 3.808 (3.664–3.888) | 1.03× | 0.89× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 87.04 (83.46–116.2) | 117.2 (95.74–132.1) | 1.35× | 0.96× | within drift |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,942 (3,751–4,096) | 4,032 (3,814–4,267) | 0.98× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 106 (102.9–111.1) | 92.67 (89.6–96.77) | 0.87× | 1.00× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,489 (4,424–5,079) | 4,719 (4,489–5,145) | 1.05× | 1.06× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 7,347 (7,050–7,772) | 7,961 (7,380–8,533) | 0.92× | 1.04× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 112.1 (110.1–115.2) | 97.79 (93.7–106) | 0.87× | 1.01× | real |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,210 (4,489–5,341) | 5,177 (4,489–5,407) | 0.99× | 1.05× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.409e+04 (1.354e+04–1.506e+04) | 1.46e+04 (1.345e+04–1.54e+04) | 0.96× | 0.98× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 143.4 (136.2–150.5) | 130.8 (123.4–140.3) | 0.91× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 4,588 (1,876–4,817) | 4,555 (1,843–4,882) | 0.99× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 2.103e+04 (1.965e+04–2.232e+04) | 2.121e+04 (1.937e+04–2.269e+04) | 0.99× | 1.03× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 203.8 (199.7–220.2) | 186.4 (183.3–201.7) | 0.91× | 0.97× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 3,015 (2,376–5,734) | 4,284 (2,245–5,669) | 1.42× | 1.12× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 2.498e+04 (2.467e+04–2.592e+04) | 2.354e+04 (2.327e+04–2.409e+04) | 1.06× | 1.43× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 610.3 (593.9–618.5) | 630.8 (626.7–651.3) | 1.03× | 1.47× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,204 (1,171–1,270) | 1,401 (1,352–1,434) | 1.16× | 1.43× | within drift |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 1.035e+06 (1.014e+06–1.047e+06) | 1.022e+06 (1.013e+06–1.03e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.672 (0.668–0.692) | 0.684 (0.676–0.692) | 1.02× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 0.924 (0.908–0.948) | 0.936 (0.932–0.964) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.898e+06 (1.878e+06–1.917e+06) | 1.893e+06 (1.886e+06–1.909e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.748 (0.74–0.756) | 0.752 (0.748–0.756) | 1.01× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.016 (0.996–1.032) | 1.016 (1.004–1.032) | 1.00× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 1.845e+06 (1.826e+06–1.854e+06) | 1.835e+06 (1.827e+06–1.846e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.78 (0.772–0.78) | 0.784 (0.78–0.788) | 1.01× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.08 (1.064–1.208) | 1.08 (1.08–1.08) | 1.00× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 1.803e+06 (1.793e+06–1.808e+06) | 1.803e+06 (1.769e+06–1.814e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 0.804 (0.796–0.804) | 0.804 (0.796–0.812) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.144 (1.128–1.16) | 1.144 (1.128–1.16) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.613e+04 (6.4e+04–7.314e+04) | 6.827e+04 (6.827e+04–6.827e+04) | 0.97× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.288 (1.24–1.32) | 1.28 (1.208–1.288) | 0.99× | 1.02× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.112 (1.704–2.896) | 2.016 (1.832–2.224) | 0.95× | 1.03× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 9.531e+04 (5.389e+04–1.078e+05) | 1.078e+05 (1.024e+05–1.138e+05) | 0.88× | 0.97× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.688 (1.608–1.768) | 1.688 (1.624–1.784) | 1.00× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 5.44 (3.984–13.38) | 4.736 (4.32–16.51) | 0.87× | 0.90× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.28e+05 (1.107e+05–1.365e+05) | 1.321e+05 (1.241e+05–1.321e+05) | 0.97× | 0.93× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 1.744 (1.688–1.752) | 1.808 (1.768–1.832) | 1.04× | 1.01× | real |
| fxfs-guest | scale-rand-write-j4-p99 | us | 4.704 (4.32–8.256) | 6.688 (5.024–9.024) | 1.42× | 1.02× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.162e+05 (1.092e+05–1.223e+05) | 1.116e+05 (9.99e+04–1.388e+05) | 1.04× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 1.752 (1.736–1.784) | 1.76 (1.704–1.816) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 6.976 (5.344–12.48) | 6.944 (5.216–8.032) | 1.00× | 0.93× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.65e+05 (1.617e+05–1.779e+05) | 1.613e+05 (1.198e+05–1.783e+05) | 1.02× | 1.04× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.249e+05 (1.939e+05–2.299e+05) | 2.213e+05 (1.767e+05–2.379e+05) | 1.02× | 1.08× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 2.014e+05 (1.749e+05–2.27e+05) | 2.016e+05 (1.613e+05–2.241e+05) | 1.00× | 0.97× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 2.079e+05 (1.991e+05–2.183e+05) | 1.971e+05 (1.751e+05–2.145e+05) | 1.05× | 0.99× | within drift |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.858e+05 (1.679e+05–2.045e+05) | 1.702e+05 (1.341e+05–1.847e+05) | 1.09× | 1.01× | within drift |
| fxfs-guest | scale-unlink-j2 | ops/s | 1.917e+05 (1.829e+05–1.945e+05) | 1.967e+05 (1.392e+05–2.153e+05) | 0.97× | 1.04× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 1.916e+05 (1.765e+05–1.963e+05) | 1.981e+05 (1.927e+05–2.077e+05) | 0.97× | 0.98× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 1.876e+05 (1.852e+05–1.929e+05) | 1.929e+05 (1.915e+05–1.99e+05) | 0.97× | 0.99× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 8.367e+04 (6.909e+04–8.542e+04) | 7.802e+04 (4.76e+04–8.705e+04) | 1.07× | 1.12× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.288e+05 (8.57e+04–1.321e+05) | 1.204e+05 (8.371e+04–1.351e+05) | 1.07× | 1.02× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.122e+05 (9.502e+04–1.283e+05) | 1.085e+05 (9.484e+04–1.149e+05) | 1.03× | 1.03× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.182e+05 (1.128e+05–1.232e+05) | 1.12e+05 (1.093e+05–1.221e+05) | 1.05× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 3,449 (3,303–4,000) | 3,844 (3,631–4,163) | 0.90× | 1.02× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 246.8 (238.6–259.1) | 229.4 (214–232.4) | 0.93× | 1.02× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 1,096 (378.9–1,614) | 699.4 (370.7–1,204) | 0.64× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.008 (2.576–3.28) | 3.04 (2.768–3.248) | 1.01× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 10.82 (6.304–14.27) | 10.3 (7.456–19.58) | 0.95× | 1.10× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 4,096 (3,793–4,452) | 4,294 (3,670–4,339) | 0.95× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 446.5 (428–452.6) | 421.9 (415.7–432.1) | 0.94× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 1,606 (1,020–2,310) | 1,360 (1,171–1,860) | 0.85× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 3.776 (3.632–4.048) | 3.872 (3.824–4.016) | 1.03× | 0.92× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 28.54 (27.26–36.61) | 29.95 (25.73–43.26) | 1.05× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 4,768 (4,686–4,865) | 5,026 (4,491–5,120) | 0.95× | 1.03× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 497.7 (493.6–501.8) | 497.7 (481.3–522.2) | 1.00× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 5,505 (5,145–5,997) | 4,686 (4,047–5,800) | 0.85× | 1.06× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 3.728 (3.664–3.856) | 3.728 (3.664–3.92) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 53.25 (34.05–57.09) | 75.78 (46.34–87.55) | 1.42× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 4,647 (4,571–4,752) | 4,935 (4,785–5,020) | 0.94× | 0.96× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 544.8 (536.6–553) | 553 (544.8–561.2) | 1.02× | 0.95× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 1.668e+04 (1.606e+04–1.796e+04) | 1.658e+04 (1.475e+04–1.769e+04) | 0.99× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 3.712 (3.632–3.952) | 3.712 (3.6–3.824) | 1.00× | 0.89× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 89.6 (63.23–107) | 90.11 (79.36–121.3) | 1.01× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,681 (5,535–6,585) | 5,721 (5,306–5,919) | 0.99× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.44 (13.25–13.76) | 13.38 (13.25–13.89) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,719 (4,555–4,882) | 4,555 (4,489–4,817) | 0.97× | 1.06× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.056e+04 (9,460–1.151e+04) | 1.061e+04 (9,638–1.198e+04) | 1.00× | 1.04× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.29 (12.1–12.35) | 12.35 (12.1–12.48) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 5,014 (4,080–5,407) | 4,981 (3,719–5,210) | 0.99× | 1.05× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.238e+04 (2.063e+04–2.354e+04) | 2.208e+04 (2.122e+04–2.417e+04) | 1.01× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.42 (12.22–12.48) | 12.48 (12.35–12.48) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,325 (3,916–4,751) | 4,489 (3,981–4,620) | 1.04× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 2.593e+04 (2.331e+04–2.768e+04) | 2.502e+04 (2.285e+04–2.673e+04) | 1.04× | 1.03× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 12.54 (12.35–12.74) | 12.48 (12.48–12.61) | 0.99× | 0.97× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 5,833 (5,341–6,849) | 6,193 (5,669–7,045) | 1.06× | 1.12× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (6.827e+05–6.827e+05) | 6.827e+05 (6.827e+05–6.827e+05) | 1.00× | 1.43× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.42 (15.17–16.19) | 15.1 (14.91–15.55) | 0.98× | 1.47× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 47.1 (37.12–68.1) | 49.41 (45.82–73.22) | 1.05× | 1.43× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 9.52e+05 (9.482e+05–9.655e+05) | 9.588e+05 (9.244e+05–9.623e+05) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.744 (0.732–0.748) | 0.744 (0.74–0.756) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 0.968 (0.956–0.98) | 0.968 (0.948–1.032) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.495e+06 (1.475e+06–1.509e+06) | 1.481e+06 (1.47e+06–1.505e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1 (0.988–1.032) | 1.02 (0.988–1.032) | 1.02× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.432 (1.416–1.464) | 1.44 (1.416–1.464) | 1.01× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 1.474e+06 (1.455e+06–1.487e+06) | 1.458e+06 (1.443e+06–1.483e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.02 (1.004–1.048) | 1.048 (1.012–1.048) | 1.03× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 1.456 (1.448–1.48) | 1.456 (1.432–1.496) | 1.00× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 1.469e+06 (1.457e+06–1.478e+06) | 1.457e+06 (1.447e+06–1.477e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 1.032 (1.02–1.048) | 1.048 (1.02–1.064) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 1.464 (1.464–1.48) | 1.472 (1.464–1.496) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 6,169 (6,024–6,522) | 7,475 (7,111–7,642) | 0.83× | 0.99× | real |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 143.4 (136.2–146.4) | 116.7 (113.2–121.3) | 0.81× | 1.02× | real |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 194.6 (177.2–207.9) | 165.9 (162.8–173.1) | 0.85× | 1.03× | real |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.174e+04 (1.125e+04–1.234e+04) | 1.33e+04 (1.191e+04–1.442e+04) | 0.88× | 0.97× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 96.77 (35.58–127.5) | 56.83 (24.96–101.9) | 0.59× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 241.7 (220.2–252.9) | 228.4 (220.2–297) | 0.94× | 0.90× | within drift |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.234e+04 (1.215e+04–1.26e+04) | 1.3e+04 (1.268e+04–1.339e+04) | 0.95× | 0.93× | within drift |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 278.5 (272.4–280.6) | 251.9 (244.7–255) | 0.90× | 1.01× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 499.7 (477.2–522.2) | 499.7 (493.6–510) | 1.00× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.137e+04 (1.121e+04–1.147e+04) | 1.206e+04 (1.175e+04–1.221e+04) | 0.94× | 0.96× | real |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 606.2 (561.2–610.3) | 577.5 (561.2–585.7) | 0.95× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,343 (1,286–1,483) | 1,188 (1,155–1,253) | 0.88× | 0.93× | real |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.603e+05 (1.576e+05–1.768e+05) | 1.626e+05 (1.585e+05–1.725e+05) | 0.99× | 1.04× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.327e+05 (2.031e+05–2.444e+05) | 2.27e+05 (1.938e+05–2.308e+05) | 1.03× | 1.08× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 1.961e+05 (1.826e+05–2.38e+05) | 2.144e+05 (1.81e+05–2.362e+05) | 0.91× | 0.97× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 2.169e+05 (2.027e+05–2.232e+05) | 2.158e+05 (2.081e+05–2.251e+05) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.556e+05 (8.958e+04–1.88e+05) | 1.785e+05 (1.301e+05–1.987e+05) | 0.87× | 1.01× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 1.892e+05 (1.578e+05–1.997e+05) | 1.789e+05 (1.771e+05–1.866e+05) | 1.06× | 1.04× | within drift |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 1.956e+05 (1.566e+05–2.012e+05) | 1.972e+05 (1.806e+05–2.047e+05) | 0.99× | 0.98× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 1.89e+05 (1.835e+05–1.938e+05) | 1.899e+05 (1.843e+05–1.922e+05) | 1.00× | 0.99× | within drift |
