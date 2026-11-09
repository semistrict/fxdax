| Case | Workload | Unit | base | landed | landed vs base | ext4-guest-kernel landed vs base | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 6.997e+04 (4.364e+04–8.693e+04) | 8.094e+04 (6.515e+04–8.672e+04) | 0.86× | 1.23× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.173e+05 (1.051e+05–1.308e+05) | 1.061e+05 (7.96e+04–1.337e+05) | 1.11× | 0.97× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.263e+05 (1.192e+05–1.31e+05) | 1.189e+05 (1.145e+05–1.27e+05) | 1.06× | 0.92× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.066e+05 (1.033e+05–1.107e+05) | 1.079e+05 (1.024e+05–1.098e+05) | 0.99× | 1.10× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 3,031 (2,738–3,180) | 3,241 (2,994–3,631) | 0.94× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 303.1 (280.6–313.3) | 259.1 (238.6–264.2) | 0.85× | 1.02× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 864.3 (481.3–1,532) | 1,614 (716.8–2,277) | 1.87× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 2.944 (2.672–3.28) | 3.056 (2.608–3.152) | 1.04× | 1.13× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 9.648 (6.368–12.86) | 12.35 (10.18–15.17) | 1.28× | 0.60× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 3,347 (2,768–3,984) | 3,782 (3,543–3,969) | 0.89× | 0.88× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 529.4 (448.5–610.3) | 458.8 (444.4–469) | 0.87× | 0.89× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 1,475 (1,155–2,998) | 1,790 (1,286–2,507) | 1.21× | 0.96× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.536 (3.024–3.984) | 3.584 (3.408–3.856) | 1.01× | 0.94× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 15.52 (12.1–29.82) | 13.82 (11.33–21.89) | 0.89× | 0.95× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 3,829 (3,737–3,923) | 4,125 (4,080–4,180) | 0.93× | 1.00× | real |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 798.7 (782.3–823.3) | 745.5 (725–774.1) | 0.93× | 1.00× | real |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 3,899 (3,752–4,751) | 4,219 (3,883–4,555) | 1.08× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 5.888 (5.728–5.984) | 5.856 (5.664–6.048) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 30.78 (25.98–42.24) | 24.45 (22.4–25.22) | 0.79× | 0.99× | real |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 4,008 (3,832–4,146) | 4,180 (4,080–4,245) | 0.96× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 1,147 (1,139–1,188) | 966.7 (921.6–1,028) | 0.84× | 1.02× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 1.212e+04 (1.094e+04–1.448e+04) | 1.553e+04 (1.435e+04–1.717e+04) | 1.28× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 6.336 (6.304–6.496) | 6.336 (6.176–6.432) | 1.00× | 0.75× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 34.56 (30.08–35.58) | 28.93 (24.96–35.07) | 0.84× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,700 (3,436–3,814) | 3,973 (3,864–4,481) | 0.93× | 0.98× | real |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 118.3 (114.2–129.5) | 101.9 (98.82–105) | 0.86× | 0.96× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,719 (4,555–4,882) | 4,751 (4,555–5,145) | 1.01× | 0.98× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 6,755 (6,512–7,250) | 7,167 (6,942–7,613) | 0.94× | 1.07× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 127.5 (124.4–129.5) | 112.6 (111.1–118.3) | 0.88× | 1.06× | real |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,112 (4,686–5,669) | 5,308 (4,882–5,603) | 1.04× | 1.03× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.04e+04 (1.029e+04–1.13e+04) | 1.128e+04 (1.074e+04–1.131e+04) | 0.92× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 214 (199.7–220.2) | 177.2 (162.8–185.3) | 0.83× | 0.98× | real |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 5,439 (2,343–5,538) | 5,636 (5,341–6,062) | 1.04× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 1.282e+04 (1.206e+04–1.342e+04) | 1.321e+04 (1.288e+04–1.356e+04) | 0.97× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 313.3 (305.2–329.7) | 294.9 (292.9–297) | 0.94× | 1.01× | real |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 7,504 (3,391–8,225) | 7,635 (6,259–8,585) | 1.02× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 2.41e+04 (2.276e+04–2.56e+04) | 2.468e+04 (2.301e+04–2.56e+04) | 0.98× | 1.08× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 569.3 (544.8–593.9) | 557.1 (528.4–610.3) | 0.98× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,745 (1,516–2,146) | 1,761 (1,630–1,991) | 1.01× | 1.18× | within drift |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 1.032e+06 (1.024e+06–1.041e+06) | 1.038e+06 (1.027e+06–1.039e+06) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.676 (0.668–0.684) | 0.668 (0.668–0.684) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 0.932 (0.924–0.948) | 0.928 (0.916–0.956) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.897e+06 (1.888e+06–1.908e+06) | 1.9e+06 (1.891e+06–1.907e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.748 (0.748–0.764) | 0.748 (0.748–0.748) | 1.00× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.032 (1.012–1.032) | 1.022 (1.012–1.096) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 3.672e+06 (3.661e+06–3.687e+06) | 3.667e+06 (3.549e+06–3.681e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.78 (0.78–0.788) | 0.784 (0.78–0.788) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.112 (1.08–1.128) | 1.128 (1.112–1.368) | 1.01× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 4.568e+06 (4.47e+06–4.597e+06) | 4.613e+06 (4.455e+06–4.625e+06) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 1.144 (1.128–1.144) | 1.128 (1.128–1.144) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.608 (1.608–1.656) | 1.608 (1.592–1.704) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.4e+04 (6.4e+04–6.827e+04) | 6.827e+04 (6.4e+04–6.827e+04) | 0.94× | 1.03× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.256 (1.24–1.272) | 1.272 (1.256–1.288) | 1.01× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.144 (1.816–2.512) | 2.032 (1.784–2.544) | 0.95× | 0.45× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 9.572e+04 (8.533e+04–1.024e+05) | 1.024e+05 (8.904e+04–1.024e+05) | 0.93× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.736 (1.688–2.16) | 1.696 (1.64–2.16) | 0.98× | 1.12× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 6.496 (5.984–15.81) | 7.424 (6.112–8.512) | 1.14× | 1.36× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.438e+05 (1.28e+05–1.575e+05) | 1.438e+05 (1.412e+05–1.638e+05) | 1.00× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 2.08 (1.736–2.288) | 2.096 (2.04–2.16) | 1.01× | 1.12× | within drift |
| fxfs-guest | scale-rand-write-j4-p99 | us | 19.33 (7.52–25.47) | 21.25 (20.1–23.17) | 1.10× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.322e+05 (1.241e+05–1.437e+05) | 1.344e+05 (1.154e+05–1.437e+05) | 0.98× | 0.97× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 2.384 (2.352–2.448) | 2.352 (2.352–2.384) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 57.6 (49.92–64.26) | 58.11 (52.99–60.67) | 1.01× | 0.98× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.646e+05 (1.216e+05–1.802e+05) | 1.541e+05 (1.21e+05–1.656e+05) | 1.07× | 1.00× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.188e+05 (1.705e+05–2.32e+05) | 2.366e+05 (2.139e+05–2.508e+05) | 0.92× | 0.95× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 2.959e+05 (2.898e+05–3.03e+05) | 3.096e+05 (2.276e+05–3.403e+05) | 0.96× | 1.03× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 3.383e+05 (3.214e+05–3.531e+05) | 3.885e+05 (3.685e+05–4.097e+05) | 0.87× | 1.06× | real |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.903e+05 (1.503e+05–2.281e+05) | 1.95e+05 (1.525e+05–2.125e+05) | 0.98× | 1.01× | within drift |
| fxfs-guest | scale-unlink-j2 | ops/s | 1.882e+05 (1.788e+05–1.917e+05) | 1.935e+05 (1.831e+05–2.096e+05) | 0.97× | 1.03× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 1.764e+05 (1.745e+05–1.821e+05) | 1.787e+05 (1.752e+05–1.806e+05) | 0.99× | 0.95× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 1.589e+05 (1.569e+05–1.602e+05) | 1.592e+05 (1.567e+05–1.661e+05) | 1.00× | 1.06× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 5.838e+04 (3.699e+04–8.64e+04) | 7.777e+04 (6.582e+04–8.831e+04) | 0.75× | 1.23× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.188e+05 (1.021e+05–1.344e+05) | 1.088e+05 (7.207e+04–1.304e+05) | 1.09× | 0.97× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.234e+05 (1.121e+05–1.354e+05) | 1.205e+05 (1.117e+05–1.417e+05) | 1.02× | 0.92× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.059e+05 (1.035e+05–1.1e+05) | 1.049e+05 (1.028e+05–1.082e+05) | 1.01× | 1.10× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 2,592 (2,473–2,681) | 3,021 (2,681–3,346) | 0.86× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 321.5 (305.2–337.9) | 261.6 (259.1–276.5) | 0.81× | 1.02× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 1,647 (946.2–2,212) | 1,409 (725–2,474) | 0.86× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.168 (3.088–3.248) | 3.104 (2.96–3.248) | 0.98× | 1.13× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 15.9 (11.46–24.19) | 12.74 (10.82–14.91) | 0.80× | 0.60× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 3,175 (2,926–3,251) | 3,814 (3,568–3,938) | 0.83× | 0.88× | real |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 553 (522.2–569.3) | 464.9 (452.6–473.1) | 0.84× | 0.89× | real |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 2,257 (1,483–2,703) | 2,007 (1,401–2,114) | 0.89× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 3.872 (3.056–4.192) | 3.696 (3.344–3.856) | 0.95× | 0.94× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 13.57 (11.58–27.26) | 12.35 (11.58–35.07) | 0.91× | 0.95× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 3,690 (3,574–3,800) | 4,039 (3,599–4,080) | 0.91× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 806.9 (798.7–831.5) | 708.6 (700.4–733.2) | 0.88× | 1.00× | real |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 4,366 (3,654–4,882) | 4,653 (4,227–6,652) | 1.07× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 5.856 (5.728–5.984) | 5.504 (5.472–5.6) | 0.94× | 0.99× | real |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 31.62 (24.19–38.66) | 23.17 (19.33–33.02) | 0.73× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 3,732 (3,647–3,824) | 4,090 (3,984–4,171) | 0.91× | 1.01× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 1,204 (1,139–1,270) | 905.2 (856.1–970.8) | 0.75× | 1.02× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 1.324e+04 (1.239e+04–1.448e+04) | 1.586e+04 (1.527e+04–1.769e+04) | 1.20× | 1.00× | real |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 6.496 (6.304–6.56) | 6.08 (5.984–6.24) | 0.94× | 0.75× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 40.96 (31.36–61.18) | 32.83 (23.68–38.14) | 0.80× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,835 (4,947–5,919) | 5,878 (5,265–6,585) | 0.99× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.31 (13.12–16.77) | 13.38 (13.12–13.5) | 1.00× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,719 (4,620–4,948) | 4,653 (4,620–4,751) | 0.99× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.088e+04 (9,615–1.28e+04) | 1.103e+04 (1.084e+04–1.249e+04) | 0.99× | 1.07× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.22 (11.97–12.35) | 12.22 (12.22–12.35) | 1.00× | 1.06× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 4,948 (2,802–5,276) | 5,112 (2,867–5,210) | 1.03× | 1.03× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.338e+04 (2.269e+04–2.49e+04) | 2.413e+04 (2.232e+04–2.453e+04) | 0.97× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.42 (12.35–12.61) | 12.35 (12.22–12.48) | 0.99× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,268 (1,860–4,686) | 4,227 (1,942–4,555) | 0.99× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 3.524e+04 (3.464e+04–3.617e+04) | 3.578e+04 (3.385e+04–3.749e+04) | 0.98× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 12.54 (12.48–12.86) | 12.61 (12.48–12.74) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 5,079 (2,703–5,210) | 5,046 (4,817–5,145) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (5.12e+05–6.827e+05) | 6.827e+05 (6.827e+05–6.827e+05) | 1.00× | 1.08× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.3 (15.17–19.84) | 15.23 (14.91–15.42) | 1.00× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 64.64 (38.14–71.17) | 49.15 (38.14–69.12) | 0.76× | 1.18× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 9.587e+05 (9.438e+05–9.694e+05) | 9.587e+05 (9.474e+05–9.708e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.74 (0.732–0.756) | 0.74 (0.732–0.756) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 0.964 (0.948–0.996) | 0.972 (0.956–1.012) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.482e+06 (1.471e+06–1.51e+06) | 1.481e+06 (1.455e+06–1.514e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1.016 (0.988–1.032) | 1.026 (0.988–1.032) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.448 (1.416–1.464) | 1.448 (1.416–1.496) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 2.511e+06 (2.499e+06–2.602e+06) | 2.495e+06 (2.465e+06–2.622e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.168 (1.144–1.176) | 1.184 (1.144–1.192) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 2.224 (2.064–2.256) | 2.256 (2.04–2.32) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 2.591e+06 (2.576e+06–2.905e+06) | 2.594e+06 (2.515e+06–2.873e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 2.512 (2.128–2.544) | 2.512 (2.192–2.608) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 5.12 (4.016–5.216) | 5.056 (4.192–5.408) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 6,282 (6,059–6,321) | 7,341 (6,781–7,817) | 0.86× | 1.03× | real |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 142.3 (140.3–144.4) | 117.2 (111.1–127.5) | 0.82× | 1.01× | real |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 209.9 (197.6–220.2) | 173.1 (164.9–187.4) | 0.82× | 0.45× | within drift |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.177e+04 (1.138e+04–1.219e+04) | 1.393e+04 (1.339e+04–1.432e+04) | 0.84× | 1.00× | real |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 101.4 (62.21–132.1) | 83.46 (60.67–93.7) | 0.82× | 1.12× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 264.2 (244.7–272.4) | 229.4 (216.1–248.8) | 0.87× | 1.36× | within drift |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.098e+04 (1.061e+04–1.154e+04) | 1.266e+04 (1.249e+04–1.284e+04) | 0.87× | 0.96× | real |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 311.3 (297–329.7) | 262.7 (257–268.3) | 0.84× | 1.12× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 577.5 (544.8–577.5) | 516.1 (489.5–522.2) | 0.89× | 1.00× | real |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.05e+04 (1.044e+04–1.061e+04) | 1.115e+04 (1.1e+04–1.124e+04) | 0.94× | 0.97× | real |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 679.9 (675.8–692.2) | 659.5 (651.3–667.6) | 0.97× | 1.00× | real |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,270 (1,253–1,286) | 1,057 (1,057–1,073) | 0.83× | 0.98× | real |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.388e+05 (1.165e+05–1.596e+05) | 1.385e+05 (1.153e+05–1.66e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.255e+05 (1.856e+05–2.345e+05) | 1.994e+05 (1.899e+05–2.272e+05) | 1.13× | 0.95× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 2.845e+05 (2.7e+05–3.038e+05) | 2.96e+05 (2.318e+05–3.214e+05) | 0.96× | 1.03× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 3.687e+05 (3.58e+05–3.901e+05) | 3.099e+05 (2.3e+05–3.792e+05) | 1.19× | 1.06× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.886e+05 (1.822e+05–2.04e+05) | 1.967e+05 (1.822e+05–2.067e+05) | 0.96× | 1.01× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 1.768e+05 (1.738e+05–1.862e+05) | 1.932e+05 (1.879e+05–2.048e+05) | 0.92× | 1.03× | real |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 1.763e+05 (1.678e+05–1.84e+05) | 1.731e+05 (1.703e+05–1.774e+05) | 1.02× | 0.95× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 1.577e+05 (1.541e+05–1.59e+05) | 1.588e+05 (1.557e+05–1.666e+05) | 0.99× | 1.06× | within drift |
