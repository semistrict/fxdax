| Case | Workload | Unit | base | landed | landed vs base | ext4-guest-kernel landed vs base | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 7.223e+04 (6.083e+04–8.494e+04) | 7.446e+04 (5.569e+04–7.963e+04) | 0.97× | 1.04× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.237e+05 (1.075e+05–1.331e+05) | 1.212e+05 (1.11e+05–1.295e+05) | 1.02× | 1.16× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.331e+05 (1.234e+05–1.42e+05) | 1.297e+05 (1.24e+05–1.432e+05) | 1.03× | 1.02× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.299e+05 (1.205e+05–1.343e+05) | 1.303e+05 (1.182e+05–1.365e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 3,106 (2,876–3,346) | 3,569 (3,368–3,793) | 0.87× | 0.95× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 290.8 (288.8–301.1) | 256 (250.9–261.1) | 0.88× | 0.96× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 701.4 (460.8–1,434) | 545.8 (407.6–831.5) | 0.78× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 3.104 (2.768–3.28) | 3.072 (2.768–3.312) | 0.99× | 1.13× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 11.07 (6.048–17.79) | 10.75 (5.344–13.63) | 0.97× | 1.02× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 3,616 (3,436–3,821) | 4,105 (3,807–4,357) | 0.88× | 1.07× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 491.5 (485.4–497.7) | 440.3 (419.8–448.5) | 0.90× | 1.01× | real |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 1,466 (1,270–2,343) | 1,171 (774.1–2,441) | 0.80× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.872 (3.6–4.08) | 3.744 (3.248–3.92) | 0.97× | 1.33× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 17.28 (12.74–20.1) | 14.27 (10.94–17.28) | 0.83× | 1.18× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 5,165 (4,935–5,292) | 5,382 (5,251–5,447) | 0.96× | 1.03× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 593.9 (585.7–602.1) | 577.5 (569.3–585.7) | 0.97× | 1.03× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 3,424 (2,966–4,555) | 3,129 (2,638–3,555) | 0.91× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 4.192 (3.952–4.512) | 4.288 (4.192–4.704) | 1.02× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 40.45 (38.14–47.36) | 34.56 (31.87–40.7) | 0.85× | 1.08× | within drift |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 5,506 (5,454–5,634) | 5,873 (5,603–6,086) | 0.94× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 794.6 (766–831.5) | 679.9 (643.1–708.6) | 0.86× | 0.98× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 9,765 (8,978–1.081e+04) | 1.042e+04 (1.029e+04–1.081e+04) | 1.07× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 4.288 (4.128–4.384) | 4.192 (3.856–4.448) | 0.98× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 120.8 (113.2–140.3) | 76.29 (63.23–88.58) | 0.63× | 1.03× | real |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,654 (3,580–4,024) | 3,882 (3,489–4,223) | 0.94× | 1.08× | within drift |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 118.3 (115.2–120.3) | 105.5 (101.9–112.1) | 0.89× | 1.09× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,915 (4,555–5,145) | 4,719 (4,489–5,145) | 0.96× | 1.09× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 6,868 (6,450–7,173) | 7,211 (6,919–7,367) | 0.95× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 128.5 (125.4–129.5) | 111.6 (110.1–114.2) | 0.87× | 1.00× | real |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,308 (4,882–5,603) | 5,243 (5,079–5,341) | 0.99× | 1.04× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.255e+04 (1.208e+04–1.381e+04) | 1.333e+04 (1.278e+04–1.435e+04) | 0.94× | 0.90× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 158.7 (154.6–164.9) | 134.1 (132.1–138.2) | 0.85× | 0.85× | real |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 5,046 (2,007–5,341) | 5,014 (2,179–5,472) | 0.99× | 0.90× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 1.878e+04 (1.833e+04–1.944e+04) | 1.972e+04 (1.923e+04–2.015e+04) | 0.95× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 202.8 (199.7–216.1) | 187.4 (179.2–199.7) | 0.92× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 6,619 (2,540–7,438) | 2,933 (2,507–7,635) | 0.44× | 1.22× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 3.08e+04 (2.844e+04–3.103e+04) | 2.969e+04 (2.626e+04–3.251e+04) | 1.04× | 1.09× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 483.3 (473.1–510) | 477.2 (452.6–528.4) | 0.99× | 1.12× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,229 (1,044–1,253) | 1,360 (1,204–1,516) | 1.11× | 1.19× | within drift |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 9.842e+05 (9.743e+05–9.94e+05) | 9.77e+05 (9.62e+05–9.91e+05) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.716 (0.708–0.732) | 0.724 (0.708–0.732) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 0.988 (0.972–1.004) | 0.988 (0.972–1.004) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.866e+06 (1.859e+06–1.88e+06) | 1.874e+06 (1.86e+06–1.879e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.772 (0.764–0.772) | 0.764 (0.764–0.772) | 0.99× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.048 (1.032–1.112) | 1.032 (1.032–1.048) | 0.98× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 3.662e+06 (3.654e+06–3.676e+06) | 3.661e+06 (3.648e+06–3.673e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.788 (0.788–0.788) | 0.788 (0.788–0.788) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.088 (1.08–1.128) | 1.112 (1.064–1.176) | 1.02× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 3.549e+06 (3.519e+06–3.581e+06) | 3.563e+06 (3.468e+06–3.586e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 0.816 (0.812–0.828) | 0.812 (0.812–0.828) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.192 (1.144–1.256) | 1.184 (1.144–1.256) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.024e+04 (6.024e+04–6.827e+04) | 6.4e+04 (5.689e+04–6.827e+04) | 0.94× | 1.03× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.384 (1.384–1.432) | 1.376 (1.368–1.416) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.32 (2.128–2.736) | 2.416 (2.192–2.768) | 1.04× | 0.43× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 1.051e+05 (9.752e+04–1.138e+05) | 9.996e+04 (5.251e+04–1.078e+05) | 1.05× | 0.95× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.784 (1.736–2.224) | 1.98 (1.672–2.224) | 1.11× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 7.008 (4.512–8.032) | 6.752 (5.344–7.712) | 0.96× | 1.19× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.49e+05 (1.365e+05–1.575e+05) | 1.546e+05 (1.321e+05–1.638e+05) | 0.96× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 2.084 (1.816–2.192) | 2.096 (1.848–2.256) | 1.01× | 1.19× | within drift |
| fxfs-guest | scale-rand-write-j4-p99 | us | 19.84 (11.58–23.17) | 19.2 (7.008–21.38) | 0.97× | 1.44× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.591e+05 (1.107e+05–1.638e+05) | 1.043e+05 (9.309e+04–1.606e+05) | 1.53× | 0.97× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 1.88 (1.832–1.944) | 1.888 (1.816–1.912) | 1.00× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 16.19 (11.58–22.14) | 14.53 (11.07–20.86) | 0.90× | 0.86× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.508e+05 (1.156e+05–1.649e+05) | 1.482e+05 (1.114e+05–1.558e+05) | 1.02× | 1.12× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.245e+05 (1.903e+05–2.413e+05) | 2.199e+05 (1.54e+05–2.416e+05) | 1.02× | 0.95× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 2.925e+05 (2.669e+05–3.067e+05) | 2.999e+05 (2.884e+05–3.26e+05) | 0.98× | 1.02× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 2.554e+05 (2.352e+05–2.829e+05) | 2.774e+05 (2.747e+05–2.855e+05) | 0.92× | 1.01× | within drift |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.978e+05 (1.723e+05–2.195e+05) | 1.81e+05 (1.659e+05–1.983e+05) | 1.09× | 0.84× | within drift |
| fxfs-guest | scale-unlink-j2 | ops/s | 1.995e+05 (1.625e+05–2.399e+05) | 1.938e+05 (1.762e+05–2.362e+05) | 1.03× | 0.94× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 2.195e+05 (1.861e+05–2.295e+05) | 2.113e+05 (1.788e+05–2.286e+05) | 1.04× | 0.96× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 2.119e+05 (1.996e+05–2.223e+05) | 2.039e+05 (1.887e+05–2.124e+05) | 1.04× | 1.04× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 7.045e+04 (6.281e+04–8.64e+04) | 7.528e+04 (6.654e+04–8.507e+04) | 0.94× | 1.04× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.144e+05 (1.058e+05–1.303e+05) | 1.193e+05 (1.02e+05–1.332e+05) | 0.96× | 1.16× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.3e+05 (1.266e+05–1.394e+05) | 1.346e+05 (1.313e+05–1.386e+05) | 0.97× | 1.02× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.293e+05 (1.242e+05–1.343e+05) | 1.336e+05 (1.248e+05–1.402e+05) | 0.97× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 3,088 (2,599–3,303) | 3,075 (2,393–3,261) | 1.00× | 0.95× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 290.8 (288.8–297) | 264.7 (250.9–284.7) | 0.91× | 0.96× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 761.9 (485.4–2,310) | 1,581 (946.2–4,047) | 2.08× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.184 (3.056–3.696) | 3.184 (2.992–3.536) | 1.00× | 1.13× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 9.728 (6.368–32.38) | 20.22 (15.81–44.8) | 2.08× | 1.02× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 3,670 (3,272–3,821) | 3,625 (3,519–3,954) | 1.01× | 1.07× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 499.7 (489.5–510) | 473.1 (456.7–493.6) | 0.95× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 1,901 (1,012–2,769) | 2,195 (1,384–2,572) | 1.16× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 4.064 (3.92–4.256) | 4.416 (3.952–5.216) | 1.09× | 1.33× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 28.03 (20.35–34.56) | 22.91 (22.4–34.56) | 0.82× | 1.18× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 4,876 (4,842–5,319) | 5,051 (4,959–5,198) | 0.97× | 1.03× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 618.5 (585.7–626.7) | 602.1 (577.5–618.5) | 0.97× | 1.03× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 3,031 (2,605–3,326) | 3,195 (2,966–3,391) | 1.05× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 4.448 (3.792–4.704) | 4.736 (4.384–4.896) | 1.06× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 40.19 (37.63–55.04) | 38.66 (36.61–40.19) | 0.96× | 1.08× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 5,275 (4,983–5,461) | 5,379 (4,692–5,520) | 0.98× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 835.6 (815.1–880.6) | 729.1 (716.8–790.5) | 0.87× | 0.98× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 9,503 (7,832–1.068e+04) | 1.042e+04 (9,241–1.317e+04) | 1.10× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 4.352 (4.128–4.64) | 4.48 (4.128–4.896) | 1.03× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 124.4 (113.2–138.2) | 83.46 (75.26–130.6) | 0.67× | 1.03× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,469 (5,251–6,132) | 5,506 (5,278–5,953) | 0.99× | 1.08× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.63 (13.38–15.3) | 13.76 (13.38–14.14) | 1.01× | 1.09× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,751 (4,686–4,817) | 4,719 (4,620–4,751) | 0.99× | 1.09× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.026e+04 (9,823–1.104e+04) | 1.034e+04 (9,894–1.268e+04) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.54 (12.1–12.61) | 12.54 (12.35–12.61) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 5,243 (4,620–5,341) | 4,882 (2,441–5,472) | 0.93× | 1.04× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.206e+04 (1.796e+04–2.381e+04) | 2.176e+04 (2.084e+04–2.374e+04) | 1.01× | 0.90× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.74 (12.61–12.74) | 12.74 (12.61–12.86) | 1.00× | 0.85× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,424 (1,909–4,751) | 4,522 (2,007–4,751) | 1.02× | 0.90× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 3.625e+04 (3.516e+04–3.873e+04) | 3.653e+04 (3.344e+04–3.929e+04) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 12.93 (12.74–13.5) | 12.99 (12.61–13.63) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 4,415 (2,277–5,014) | 4,751 (2,245–5,145) | 1.08× | 1.22× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (6.827e+05–6.827e+05) | 6.827e+05 (5.12e+05–6.827e+05) | 1.00× | 1.09× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.55 (15.17–15.81) | 15.55 (15.17–19.58) | 1.00× | 1.12× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 58.37 (46.85–72.19) | 64.9 (50.43–83.46) | 1.11× | 1.19× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 9.14e+05 (9.07e+05–9.232e+05) | 9.114e+05 (8.877e+05–9.151e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.788 (0.78–0.804) | 0.796 (0.796–0.828) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 1.02 (1.004–1.032) | 1.026 (1.012–1.064) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.485e+06 (1.456e+06–1.49e+06) | 1.472e+06 (1.461e+06–1.487e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1.012 (1.004–1.048) | 1.032 (1.012–1.048) | 1.02× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.464 (1.432–1.496) | 1.456 (1.432–1.48) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 2.591e+06 (2.445e+06–2.614e+06) | 2.526e+06 (2.512e+06–2.601e+06) | 1.03× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.144 (1.144–1.208) | 1.176 (1.144–1.176) | 1.03× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 2.096 (1.992–2.352) | 2.16 (2.096–2.224) | 1.03× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 2.529e+06 (2.465e+06–2.611e+06) | 2.517e+06 (2.5e+06–2.576e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 1.16 (1.144–1.192) | 1.184 (1.16–1.192) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 2.128 (2.008–2.32) | 2.192 (2.128–2.224) | 1.03× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 6,206 (6,059–6,522) | 7,065 (6,693–7,529) | 0.88× | 1.03× | real |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 141.3 (136.2–146.4) | 122.9 (114.2–128.5) | 0.87× | 0.99× | real |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 207.9 (189.4–240.6) | 184.3 (169–197.6) | 0.89× | 0.43× | within drift |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.164e+04 (1.131e+04–1.272e+04) | 1.335e+04 (1.304e+04–1.432e+04) | 0.87× | 0.95× | real |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 120.8 (67.07–132.1) | 106 (96.77–109.1) | 0.88× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 254 (246.8–288.8) | 237.6 (230.4–346.1) | 0.94× | 1.19× | within drift |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.05e+04 (1.032e+04–1.104e+04) | 1.237e+04 (1.223e+04–1.26e+04) | 0.85× | 0.96× | real |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 329.7 (313.3–333.8) | 272.4 (268.3–276.5) | 0.83× | 1.19× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 585.7 (553–618.5) | 528.4 (510–536.6) | 0.90× | 1.44× | within drift |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.046e+04 (1.032e+04–1.056e+04) | 1.088e+04 (1.076e+04–1.101e+04) | 0.96× | 0.97× | real |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 708.6 (692.2–716.8) | 679.9 (659.5–684) | 0.96× | 1.01× | real |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,106 (1,090–1,122) | 1,081 (1,044–1,155) | 0.98× | 0.86× | within drift |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.36e+05 (1.215e+05–1.496e+05) | 1.271e+05 (1.14e+05–1.589e+05) | 1.07× | 1.12× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.169e+05 (2.027e+05–2.309e+05) | 2.262e+05 (1.867e+05–2.359e+05) | 0.96× | 0.95× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 3.014e+05 (2.982e+05–3.387e+05) | 3.03e+05 (2.965e+05–3.36e+05) | 0.99× | 1.02× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 2.906e+05 (2.824e+05–2.968e+05) | 2.887e+05 (2.821e+05–3e+05) | 1.01× | 1.01× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.917e+05 (1.584e+05–2.222e+05) | 1.851e+05 (1.305e+05–2.228e+05) | 1.04× | 0.84× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 2.028e+05 (1.813e+05–2.351e+05) | 2.069e+05 (1.794e+05–2.236e+05) | 0.98× | 0.94× | within drift |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 1.996e+05 (1.611e+05–2.237e+05) | 2.113e+05 (1.823e+05–2.305e+05) | 0.94× | 0.96× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 2.049e+05 (1.973e+05–2.081e+05) | 2.026e+05 (1.769e+05–2.045e+05) | 1.01× | 1.04× | within drift |
