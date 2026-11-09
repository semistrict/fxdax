| Case | Workload | Unit | unaffine | affine | affine vs unaffine | ext4-guest-kernel affine vs unaffine | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 7.885e+04 (5.9e+04–8.607e+04) | 7.839e+04 (5.625e+04–8.445e+04) | 1.01× | 0.92× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.287e+05 (9.863e+04–1.31e+05) | 1.096e+05 (8.868e+04–1.247e+05) | 1.17× | 1.11× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.33e+05 (1.31e+05–1.353e+05) | 1.309e+05 (1.266e+05–1.369e+05) | 1.02× | 1.06× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.289e+05 (1.185e+05–1.427e+05) | 1.318e+05 (1.184e+05–1.371e+05) | 0.98× | 0.96× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 3,063 (2,768–3,282) | 3,534 (3,103–3,793) | 0.87× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 292.9 (288.8–297) | 257 (244.7–272.4) | 0.88× | 0.99× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 660.5 (419.8–1,090) | 558.1 (407.6–946.2) | 0.84× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 3.12 (2.864–3.184) | 3.232 (2.864–3.536) | 1.04× | 1.06× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 9.984 (5.536–13.76) | 12.86 (5.472–19.58) | 1.29× | 1.25× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 3,604 (3,391–3,807) | 4,072 (3,737–4,357) | 0.89× | 0.93× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 491.5 (489.5–501.8) | 450.6 (436.2–460.8) | 0.92× | 0.96× | real |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 1,872 (1,221–2,605) | 1,122 (749.6–1,253) | 0.60× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.952 (3.696–4.128) | 4 (3.536–4.128) | 1.01× | 0.52× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 13.06 (12.22–15.81) | 15.52 (11.2–21.63) | 1.19× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 4,960 (4,763–5,265) | 5,440 (5,251–5,769) | 0.91× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 610.3 (593.9–626.7) | 565.2 (536.6–602.1) | 0.93× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 3,719 (2,736–5,014) | 3,015 (2,703–3,097) | 0.81× | 0.96× | within drift |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 4.288 (4.128–4.64) | 4.104 (3.696–4.768) | 0.96× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 41.22 (34.05–45.82) | 32.7 (30.59–41.22) | 0.79× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 5,650 (5,603–5,689) | 5,689 (5,535–5,962) | 0.99× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 811 (782.3–839.7) | 716.8 (684–716.8) | 0.88× | 1.00× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 9,306 (8,847–9,765) | 1.068e+04 (9,634–1.16e+04) | 1.15× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 4.288 (4.128–4.576) | 4.288 (4.048–4.448) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 115.2 (100.9–129.5) | 83.46 (70.14–101.9) | 0.72× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,631 (3,459–3,879) | 3,807 (3,683–3,969) | 0.95× | 0.88× | within drift |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 119.8 (118.3–127.5) | 105.5 (100.9–112.1) | 0.88× | 0.96× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,686 (4,555–5,145) | 4,686 (4,620–4,817) | 1.00× | 0.82× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 6,749 (6,481–7,250) | 7,050 (6,966–7,628) | 0.96× | 1.06× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 127.5 (126.5–130.6) | 113.7 (109.1–116.2) | 0.89× | 1.01× | real |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,439 (5,079–5,669) | 5,341 (5,079–5,538) | 0.98× | 1.11× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.215e+04 (1.198e+04–1.26e+04) | 1.344e+04 (1.288e+04–1.43e+04) | 0.90× | 1.36× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 159.7 (154.6–162.8) | 137.2 (128.5–142.3) | 0.86× | 1.68× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 5,308 (5,210–5,407) | 5,243 (2,540–5,472) | 0.99× | 1.21× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 1.858e+04 (1.798e+04–2.045e+04) | 1.947e+04 (1.808e+04–2.053e+04) | 0.95× | 0.90× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 204.8 (199.7–209.9) | 189.4 (185.3–201.7) | 0.92× | 0.97× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 7,340 (2,376–7,635) | 2,834 (2,474–8,454) | 0.39× | 0.98× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 2.968e+04 (2.926e+04–3.2e+04) | 2.768e+04 (2.731e+04–3.012e+04) | 1.07× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 493.6 (460.8–505.9) | 493.6 (469–528.4) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,163 (1,036–1,237) | 1,401 (1,319–1,565) | 1.20× | 1.09× | real |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 9.559e+05 (9.385e+05–9.643e+05) | 9.651e+05 (9.501e+05–9.906e+05) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.748 (0.74–0.764) | 0.736 (0.708–0.756) | 0.98× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 1.016 (1.004–1.032) | 1.008 (0.964–1.012) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.847e+06 (1.824e+06–1.862e+06) | 1.851e+06 (1.842e+06–1.859e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.78 (0.772–0.796) | 0.78 (0.764–0.78) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.072 (1.048–1.128) | 1.072 (1.064–1.128) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 3.617e+06 (3.589e+06–3.647e+06) | 3.628e+06 (3.585e+06–3.667e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.8 (0.796–0.812) | 0.796 (0.788–0.804) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.136 (1.096–1.208) | 1.128 (1.064–1.144) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 3.507e+06 (3.386e+06–3.523e+06) | 3.534e+06 (3.508e+06–3.587e+06) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 0.828 (0.828–0.868) | 0.82 (0.812–0.828) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.208 (1.176–1.288) | 1.192 (1.144–1.224) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.212e+04 (5.689e+04–6.827e+04) | 6.4e+04 (6.4e+04–6.827e+04) | 0.97× | 0.98× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.376 (1.352–1.448) | 1.376 (1.352–1.448) | 1.00× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.528 (2.32–3.152) | 2.464 (2.128–2.928) | 0.97× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 1.051e+05 (9.752e+04–1.078e+05) | 1.024e+05 (5.535e+04–1.078e+05) | 1.03× | 1.02× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.768 (1.752–1.784) | 1.784 (1.736–2.16) | 1.01× | 1.18× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 5.984 (4.704–7.328) | 7.104 (6.624–8.384) | 1.19× | 0.76× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.517e+05 (1.463e+05–1.638e+05) | 1.546e+05 (1.517e+05–1.638e+05) | 0.98× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 1.808 (1.784–2.192) | 1.856 (1.832–2.192) | 1.03× | 0.98× | within drift |
| fxfs-guest | scale-rand-write-j4-p99 | us | 12.03 (6.944–21.38) | 11.58 (6.944–27.01) | 0.96× | 1.12× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.308e+05 (1.064e+05–1.575e+05) | 1.284e+05 (9.309e+04–1.606e+05) | 1.02× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 1.864 (1.816–1.928) | 1.912 (1.848–2.128) | 1.03× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 17.76 (11.46–29.31) | 17.06 (7.712–53.5) | 0.96× | 0.64× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.616e+05 (1.265e+05–1.751e+05) | 1.445e+05 (1.08e+05–1.641e+05) | 1.12× | 1.03× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.287e+05 (2.121e+05–2.391e+05) | 2.19e+05 (2.035e+05–2.302e+05) | 1.04× | 1.04× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 2.86e+05 (2.618e+05–3.223e+05) | 3.053e+05 (2.843e+05–3.19e+05) | 0.94× | 1.00× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 2.793e+05 (2.548e+05–3.008e+05) | 2.829e+05 (2.765e+05–2.942e+05) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.901e+05 (1.767e+05–2.14e+05) | 1.899e+05 (1.752e+05–2.387e+05) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-unlink-j2 | ops/s | 2.05e+05 (1.962e+05–2.307e+05) | 2.067e+05 (1.828e+05–2.404e+05) | 0.99× | 1.07× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 1.937e+05 (1.783e+05–2.218e+05) | 2.119e+05 (1.958e+05–2.186e+05) | 0.91× | 0.96× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 1.995e+05 (1.802e+05–2.122e+05) | 2.028e+05 (1.942e+05–2.068e+05) | 0.98× | 0.99× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 7.471e+04 (6.717e+04–8.492e+04) | 7.814e+04 (6.595e+04–8.617e+04) | 0.96× | 0.92× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.218e+05 (1.166e+05–1.345e+05) | 1.151e+05 (1.045e+05–1.314e+05) | 1.06× | 1.11× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.328e+05 (1.258e+05–1.44e+05) | 1.286e+05 (1.151e+05–1.454e+05) | 1.03× | 1.06× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.315e+05 (1.294e+05–1.418e+05) | 1.304e+05 (1.245e+05–1.35e+05) | 1.01× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 3,083 (2,876–3,303) | 3,142 (2,943–3,606) | 0.98× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 294.9 (288.8–301.1) | 268.3 (264.2–268.3) | 0.91× | 0.99× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 761.9 (464.9–1,630) | 1,284 (444.4–1,942) | 1.69× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.2 (3.088–3.312) | 3.264 (3.184–3.44) | 1.02× | 1.06× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 11.97 (8.384–23.17) | 12.8 (11.2–19.58) | 1.07× | 1.25× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 3,420 (3,251–3,644) | 3,684 (3,568–3,879) | 0.93× | 0.93× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 518.1 (505.9–522.2) | 466.9 (464.9–485.4) | 0.90× | 0.96× | real |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 2,224 (1,597–2,638) | 2,028 (1,221–2,441) | 0.91× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 4.224 (3.984–4.448) | 4.136 (4.016–4.64) | 0.98× | 0.52× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 27.39 (14.02–29.57) | 26.88 (17.79–31.62) | 0.98× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 5,095 (4,708–5,146) | 5,139 (4,995–5,251) | 0.99× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 606.2 (602.1–634.9) | 589.8 (577.5–610.3) | 0.97× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 2,933 (2,474–3,228) | 3,080 (2,966–3,817) | 1.05× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 4.384 (4.192–4.704) | 4.512 (4.192–4.96) | 1.03× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 40.19 (36.1–46.85) | 35.84 (31.36–38.66) | 0.89× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 5,268 (5,026–5,642) | 5,476 (5,397–5,498) | 0.96× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 852 (782.3–888.8) | 741.4 (684–782.3) | 0.87× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 8,913 (7,635–1.029e+04) | 1.003e+04 (8,716–1.134e+04) | 1.12× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 4.32 (4.128–4.64) | 4.448 (4.128–4.704) | 1.03× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 121.9 (83.46–138.2) | 80.9 (69.12–103.9) | 0.66× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,418 (5,120–5,657) | 5,404 (4,842–5,689) | 1.00× | 0.88× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.89 (13.76–13.89) | 14.08 (13.63–14.78) | 1.01× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,817 (4,686–4,882) | 4,817 (4,686–4,948) | 1.00× | 0.82× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.033e+04 (9,942–1.116e+04) | 1.088e+04 (1.016e+04–1.154e+04) | 0.95× | 1.06× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.54 (12.22–12.86) | 12.48 (12.35–12.61) | 0.99× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 5,014 (4,686–5,407) | 5,177 (4,145–5,341) | 1.03× | 1.11× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.233e+04 (2.084e+04–2.321e+04) | 2.266e+04 (2.09e+04–2.295e+04) | 0.99× | 1.36× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.74 (12.48–13.12) | 12.67 (12.61–12.74) | 0.99× | 1.68× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,301 (4,014–4,817) | 4,456 (4,293–4,555) | 1.04× | 1.21× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 3.711e+04 (3.378e+04–3.766e+04) | 3.711e+04 (3.206e+04–3.81e+04) | 1.00× | 0.90× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 12.99 (12.74–13.38) | 12.99 (12.86–13.25) | 1.00× | 0.97× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 5,014 (2,441–5,210) | 2,195 (2,089–4,817) | 0.44× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (6.827e+05–6.827e+05) | 6.827e+05 (5.12e+05–6.827e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.74 (15.42–15.94) | 15.68 (15.04–20.1) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 60.67 (50.94–85.5) | 59.14 (50.94–71.17) | 0.97× | 1.09× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 8.896e+05 (8.773e+05–8.943e+05) | 8.949e+05 (8.68e+05–9.009e+05) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.828 (0.82–0.844) | 0.816 (0.812–0.86) | 0.99× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 1.048 (1.048–1.064) | 1.048 (1.032–1.08) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.461e+06 (1.446e+06–1.474e+06) | 1.473e+06 (1.456e+06–1.486e+06) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1.032 (1.02–1.048) | 1.02 (1.012–1.048) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.488 (1.464–1.512) | 1.464 (1.432–1.48) | 0.98× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 2.525e+06 (2.419e+06–2.575e+06) | 2.578e+06 (2.518e+06–2.617e+06) | 0.98× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.176 (1.16–1.224) | 1.16 (1.144–1.176) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 2.16 (2.128–2.352) | 2.112 (2.024–2.192) | 0.98× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 2.495e+06 (2.469e+06–2.561e+06) | 2.561e+06 (2.5e+06–2.606e+06) | 0.97× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 1.192 (1.176–1.208) | 1.16 (1.16–1.192) | 0.97× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 2.208 (2.128–2.288) | 2.128 (2.024–2.192) | 0.96× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 5,937 (5,476–6,400) | 6,896 (6,481–7,367) | 0.86× | 0.98× | real |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 151.6 (136.2–166.9) | 126 (115.2–140.3) | 0.83× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 217.1 (203.8–222.2) | 178.2 (173.1–191.5) | 0.82× | 0.99× | real |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.116e+04 (1.061e+04–1.184e+04) | 1.301e+04 (1.198e+04–1.403e+04) | 0.86× | 1.02× | real |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 123.4 (75.26–134.1) | 90.62 (72.19–113.2) | 0.73× | 1.18× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 268.3 (240.6–284.7) | 235.5 (228.4–242.7) | 0.88× | 0.76× | within drift |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.034e+04 (1.027e+04–1.04e+04) | 1.177e+04 (1.174e+04–1.18e+04) | 0.88× | 0.99× | real |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 337.9 (333.8–342) | 284.7 (280.6–284.7) | 0.84× | 0.98× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 585.7 (569.3–610.3) | 548.9 (544.8–569.3) | 0.94× | 1.12× | within drift |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.02e+04 (1.01e+04–1.028e+04) | 1.058e+04 (7,817–1.069e+04) | 0.96× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 716.8 (708.6–716.8) | 692.2 (684–700.4) | 0.97× | 1.01× | real |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,188 (1,171–1,221) | 1,147 (1,122–6,062) | 0.97× | 0.64× | within drift |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.444e+05 (1.131e+05–1.562e+05) | 1.308e+05 (1.138e+05–1.555e+05) | 1.10× | 1.03× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.109e+05 (1.574e+05–2.341e+05) | 2.107e+05 (2.028e+05–2.382e+05) | 1.00× | 1.04× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 2.966e+05 (2.827e+05–3.313e+05) | 2.909e+05 (2.804e+05–3.123e+05) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 2.741e+05 (2.572e+05–2.873e+05) | 2.793e+05 (2.68e+05–2.95e+05) | 0.98× | 1.00× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.953e+05 (1.876e+05–2.201e+05) | 1.807e+05 (1.522e+05–2.254e+05) | 1.08× | 0.99× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 2.075e+05 (1.792e+05–2.381e+05) | 2.053e+05 (1.873e+05–2.321e+05) | 1.01× | 1.07× | within drift |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 2.117e+05 (1.866e+05–2.226e+05) | 2.103e+05 (2.063e+05–2.181e+05) | 1.01× | 0.96× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 1.931e+05 (1.744e+05–2.061e+05) | 1.975e+05 (1.89e+05–2.077e+05) | 0.98× | 0.99× | within drift |
