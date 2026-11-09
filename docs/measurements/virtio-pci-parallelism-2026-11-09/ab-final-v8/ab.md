| Case | Workload | Unit | base | after | after vs base | ext4-guest-kernel after vs base | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 7.803e+04 (3.603e+04–8.574e+04) | 7.623e+04 (4.031e+04–8.631e+04) | 1.02× | 1.08× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.223e+05 (8.494e+04–1.335e+05) | 1.133e+05 (6.483e+04–1.318e+05) | 1.08× | 1.09× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.22e+05 (1.147e+05–1.285e+05) | 1.199e+05 (1.164e+05–1.259e+05) | 1.02× | 1.00× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.061e+05 (9.629e+04–1.094e+05) | 1.048e+05 (1.002e+05–1.071e+05) | 1.01× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 2,814 (2,738–3,220) | 3,210 (3,048–3,391) | 0.88× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 307.2 (297–309.2) | 259.1 (250.9–264.2) | 0.84× | 1.01× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 1,073 (428–1,958) | 1,194 (700.4–2,408) | 1.11× | 1.01× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 3.056 (2.736–3.28) | 3.104 (2.896–3.28) | 1.02× | 1.06× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 10.56 (7.776–15.68) | 13.63 (11.46–19.58) | 1.29× | 1.21× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 3,180 (3,012–3,436) | 3,656 (3,471–4,031) | 0.87× | 1.02× | real |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 561.2 (522.2–577.5) | 475.1 (444.4–485.4) | 0.85× | 1.37× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 1,384 (1,303–2,408) | 1,921 (1,057–2,572) | 1.39× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.728 (3.184–4.192) | 3.744 (2.96–4.016) | 1.00× | 4.35× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 13.06 (11.46–42.24) | 13.7 (10.69–37.63) | 1.05× | 1.03× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 3,768 (3,683–3,857) | 3,965 (3,842–4,205) | 0.95× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 815.1 (798.7–847.9) | 766 (741.4–782.3) | 0.94× | 0.99× | real |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 4,162 (3,523–4,620) | 4,268 (3,621–4,620) | 1.03× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 5.76 (5.664–5.92) | 5.824 (5.728–6.24) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 30.59 (22.4–34.05) | 23.81 (22.4–32.38) | 0.78× | 1.02× | within drift |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 3,933 (3,303–3,984) | 4,090 (3,950–4,227) | 0.96× | 0.94× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 1,180 (1,139–1,237) | 966.7 (938–1,036) | 0.82× | 1.00× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 1.35e+04 (1.226e+04–1.743e+04) | 1.54e+04 (1.435e+04–1.691e+04) | 1.14× | 0.91× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 6.304 (6.176–6.688) | 6.24 (5.984–6.432) | 0.99× | 2.05× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 35.58 (31.36–62.21) | 27.52 (26.5–29.82) | 0.77× | 0.96× | real |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,664 (3,419–3,916) | 3,916 (3,677–4,047) | 0.94× | 1.07× | within drift |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 119.8 (114.2–123.4) | 105 (103.9–113.2) | 0.88× | 1.05× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,784 (4,555–5,145) | 4,653 (4,555–5,276) | 0.97× | 1.15× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 6,577 (5,997–6,759) | 7,075 (6,884–7,862) | 0.93× | 0.98× | real |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 128.5 (126.5–140.3) | 114.7 (111.1–119.3) | 0.89× | 0.95× | real |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,636 (5,472–5,865) | 5,439 (3,621–5,669) | 0.97× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.019e+04 (9,918–1.06e+04) | 1.102e+04 (1.086e+04–1.112e+04) | 0.92× | 0.92× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 217.1 (205.8–224.3) | 181.2 (162.8–191.5) | 0.83× | 0.84× | real |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 5,407 (2,998–5,931) | 5,636 (4,358–6,259) | 1.04× | 0.98× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 1.269e+04 (1.203e+04–1.29e+04) | 1.308e+04 (1.291e+04–1.355e+04) | 0.97× | 1.01× | real |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 315.4 (313.3–325.6) | 297 (284.7–301.1) | 0.94× | 1.01× | real |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 7,635 (3,654–8,585) | 7,602 (4,489–8,585) | 1.00× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 2.395e+04 (2.276e+04–2.438e+04) | 2.483e+04 (2.354e+04–2.56e+04) | 0.96× | 0.79× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 573.4 (561.2–585.7) | 557.1 (536.6–569.3) | 0.97× | 0.72× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,737 (1,630–2,056) | 1,745 (1,597–1,876) | 1.00× | 0.79× | within drift |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 9.713e+05 (9.563e+05–9.919e+05) | 9.801e+05 (9.666e+05–9.854e+05) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.732 (0.716–0.74) | 0.724 (0.716–0.732) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 1 (0.98–1.032) | 0.992 (0.988–1.02) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.859e+06 (1.85e+06–1.866e+06) | 1.867e+06 (1.853e+06–1.875e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.772 (0.772–0.772) | 0.772 (0.764–0.772) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.064 (1.048–1.144) | 1.064 (1.048–1.112) | 1.00× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 3.638e+06 (3.619e+06–3.653e+06) | 3.642e+06 (3.639e+06–3.655e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.792 (0.788–0.796) | 0.788 (0.788–0.796) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.128 (1.096–1.144) | 1.128 (1.128–1.24) | 1.00× | 1.01× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 4.554e+06 (4.465e+06–4.588e+06) | 4.574e+06 (4.558e+06–4.591e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 1.152 (1.144–1.176) | 1.144 (1.144–1.144) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.64 (1.624–1.672) | 1.624 (1.624–1.64) | 0.99× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.024e+04 (5.689e+04–6.4e+04) | 6.613e+04 (6.024e+04–6.827e+04) | 0.91× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.384 (1.368–1.4) | 1.392 (1.368–1.416) | 1.01× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.336 (2.256–2.608) | 2.704 (2.256–5.664) | 1.16× | 0.44× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 1.024e+05 (8.904e+04–1.078e+05) | 1.051e+05 (9.309e+04–1.138e+05) | 0.97× | 0.92× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.696 (1.672–2.192) | 1.704 (1.672–2.16) | 1.00× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 7.168 (6.368–22.66) | 12.54 (6.688–26.5) | 1.75× | 0.82× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.517e+05 (1.205e+05–1.575e+05) | 1.414e+05 (1.321e+05–1.575e+05) | 1.07× | 0.94× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 2.096 (1.752–2.128) | 2.144 (1.816–2.32) | 1.02× | 1.02× | within drift |
| fxfs-guest | scale-rand-write-j4-p99 | us | 20.99 (17.28–37.63) | 22.91 (9.024–34.05) | 1.09× | 0.96× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.3e+05 (9.99e+04–1.437e+05) | 1.367e+05 (9.87e+04–1.489e+05) | 0.95× | 0.95× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 2.384 (2.352–2.448) | 2.384 (2.384–2.384) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 50.94 (48.9–58.62) | 58.88 (55.55–60.67) | 1.16× | 1.32× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.485e+05 (1.177e+05–1.607e+05) | 1.545e+05 (1.194e+05–1.681e+05) | 0.96× | 1.12× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.318e+05 (2.134e+05–2.359e+05) | 2.331e+05 (2.214e+05–2.451e+05) | 0.99× | 1.10× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 3.022e+05 (2.798e+05–3.325e+05) | 3.161e+05 (2.209e+05–3.396e+05) | 0.96× | 1.03× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 3.419e+05 (3.327e+05–3.626e+05) | 3.912e+05 (2.562e+05–4.007e+05) | 0.87× | 1.06× | within drift |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.952e+05 (1.609e+05–2.107e+05) | 1.772e+05 (1.515e+05–2.041e+05) | 1.10× | 0.91× | within drift |
| fxfs-guest | scale-unlink-j2 | ops/s | 1.821e+05 (1.661e+05–1.907e+05) | 1.887e+05 (1.782e+05–1.992e+05) | 0.97× | 0.97× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 1.751e+05 (1.684e+05–1.798e+05) | 1.753e+05 (1.686e+05–1.854e+05) | 1.00× | 1.08× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 1.549e+05 (1.509e+05–1.562e+05) | 1.571e+05 (1.516e+05–1.702e+05) | 0.99× | 1.05× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 7.781e+04 (6.463e+04–8.673e+04) | 7.725e+04 (5.293e+04–8.761e+04) | 1.01× | 1.08× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.096e+05 (9.378e+04–1.176e+05) | 1.114e+05 (8.099e+04–1.18e+05) | 0.98× | 1.09× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.211e+05 (1.164e+05–1.255e+05) | 1.197e+05 (1.086e+05–1.284e+05) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.043e+05 (9.618e+04–1.091e+05) | 1.067e+05 (1.032e+05–1.091e+05) | 0.98× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 2,554 (1,610–2,695) | 3,069 (2,681–3,180) | 0.83× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 329.7 (309.2–342) | 264.2 (261.1–272.4) | 0.80× | 1.01× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 1,671 (1,466–7,569) | 1,270 (938–2,671) | 0.76× | 1.01× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.232 (3.12–3.792) | 3.216 (3.12–3.344) | 1.00× | 1.06× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 29.57 (16.32–42.24) | 14.66 (12.22–18.56) | 0.50× | 1.21× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 3,052 (2,968–3,531) | 3,540 (3,230–3,724) | 0.86× | 1.02× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 585.7 (497.7–610.3) | 481.3 (473.1–497.7) | 0.82× | 1.37× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 2,261 (1,630–2,834) | 1,839 (1,401–3,588) | 0.81× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 4 (3.12–4.192) | 3.776 (3.376–3.92) | 0.94× | 4.35× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 20.99 (12.1–42.75) | 21.25 (12.61–39.17) | 1.01× | 1.03× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 3,590 (3,459–3,690) | 3,871 (3,625–3,886) | 0.93× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 815.1 (790.5–831.5) | 741.4 (733.2–766) | 0.91× | 0.99× | real |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 4,424 (4,227–4,882) | 4,817 (4,047–5,931) | 1.09× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 5.728 (5.664–5.92) | 5.696 (5.664–5.92) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 29.7 (24.45–46.34) | 24.83 (21.63–46.34) | 0.84× | 1.02× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 3,685 (3,615–3,800) | 3,929 (3,916–3,935) | 0.94× | 0.94× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 1,155 (1,106–1,171) | 1,008 (970.8–1,044) | 0.87× | 1.00× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 1.468e+04 (1.291e+04–1.632e+04) | 1.566e+04 (1.422e+04–1.632e+04) | 1.07× | 0.91× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 6.368 (6.048–6.496) | 6.24 (6.112–6.304) | 0.98× | 2.05× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 39.68 (32.38–54.02) | 30.21 (27.01–32.38) | 0.76× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,570 (5,159–6,077) | 5,844 (5,432–6,024) | 0.95× | 1.07× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.95 (13.63–15.04) | 13.63 (13.38–14.14) | 0.98× | 1.05× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,751 (4,686–4,882) | 4,751 (4,751–4,817) | 1.00× | 1.15× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.096e+04 (1.024e+04–1.268e+04) | 1.107e+04 (1.021e+04–1.226e+04) | 0.99× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.48 (12.35–12.61) | 12.48 (12.35–12.61) | 1.00× | 0.95× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 4,784 (2,441–5,210) | 4,784 (2,540–5,276) | 1.00× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.314e+04 (2.185e+04–2.626e+04) | 2.229e+04 (2.19e+04–2.424e+04) | 1.04× | 0.92× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.67 (12.48–12.86) | 12.54 (12.35–12.74) | 0.99× | 0.84× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,219 (2,007–4,620) | 4,489 (4,358–4,555) | 1.06× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 3.498e+04 (3.264e+04–3.569e+04) | 3.52e+04 (3.2e+04–3.649e+04) | 0.99× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 12.74 (12.48–13.12) | 12.74 (12.48–12.99) | 1.00× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 5,112 (2,638–5,210) | 5,046 (4,948–5,407) | 0.99× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (6.827e+05–6.827e+05) | 6.827e+05 (5.12e+05–6.827e+05) | 1.00× | 0.79× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.81 (15.42–16.06) | 15.81 (15.3–19.84) | 1.00× | 0.72× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 77.31 (59.65–90.62) | 63.87 (55.55–81.41) | 0.83× | 0.79× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 9.071e+05 (9.033e+05–9.174e+05) | 9.083e+05 (9.033e+05–9.15e+05) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.8 (0.788–0.804) | 0.8 (0.796–0.804) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 1.032 (1.012–1.08) | 1.02 (1.02–1.08) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.472e+06 (1.464e+06–1.478e+06) | 1.477e+06 (1.451e+06–1.486e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1.026 (1.02–1.032) | 1.02 (1.012–1.048) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.464 (1.464–1.48) | 1.464 (1.448–1.48) | 1.00× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 2.54e+06 (2.528e+06–2.611e+06) | 2.566e+06 (2.468e+06–2.599e+06) | 0.99× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.16 (1.144–1.176) | 1.16 (1.144–1.208) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 2.16 (2.024–2.192) | 2.112 (2.064–2.32) | 0.98× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 2.707e+06 (2.687e+06–2.959e+06) | 2.848e+06 (2.531e+06–2.887e+06) | 0.95× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 2.352 (2.064–2.384) | 2.192 (2.16–2.608) | 0.93× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 4.8 (4.016–4.832) | 4.256 (4.08–5.344) | 0.89× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 6,321 (6,024–6,400) | 7,237 (6,440–7,474) | 0.87× | 0.99× | real |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 140.3 (136.2–146.4) | 122.9 (115.2–134.1) | 0.88× | 1.01× | real |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 213 (193.5–230.4) | 180.2 (171–201.7) | 0.85× | 0.44× | within drift |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.157e+04 (1.138e+04–1.198e+04) | 1.343e+04 (1.249e+04–1.365e+04) | 0.86× | 0.92× | real |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 117.2 (76.29–134.1) | 78.34 (68.1–108) | 0.67× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 292.9 (264.2–301.1) | 230.4 (220.2–240.6) | 0.79× | 0.82× | real |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.063e+04 (1.045e+04–1.101e+04) | 1.13e+04 (1.078e+04–1.144e+04) | 0.94× | 0.94× | within drift |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 315.4 (309.2–329.7) | 294.9 (292.9–305.2) | 0.94× | 1.02× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 573.4 (536.6–593.9) | 598 (577.5–643.1) | 1.04× | 0.96× | within drift |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.04e+04 (1.034e+04–1.071e+04) | 1.071e+04 (1.045e+04–1.094e+04) | 0.97× | 0.95× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 692.2 (675.8–700.4) | 692.2 (675.8–700.4) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,237 (1,221–1,303) | 1,114 (1,090–1,237) | 0.90× | 1.32× | within drift |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.427e+05 (1.243e+05–1.583e+05) | 1.402e+05 (1.147e+05–1.622e+05) | 1.02× | 1.12× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.193e+05 (2.057e+05–2.326e+05) | 2.26e+05 (1.876e+05–2.338e+05) | 0.97× | 1.10× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 2.679e+05 (2.284e+05–2.796e+05) | 2.884e+05 (2.703e+05–2.933e+05) | 0.93× | 1.03× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 3.655e+05 (3.417e+05–3.733e+05) | 3.625e+05 (2.861e+05–3.889e+05) | 1.01× | 1.06× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.787e+05 (1.67e+05–2.045e+05) | 1.821e+05 (1.53e+05–2.134e+05) | 0.98× | 0.91× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 1.83e+05 (1.635e+05–1.916e+05) | 1.796e+05 (1.731e+05–1.889e+05) | 1.02× | 0.97× | within drift |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 1.73e+05 (1.659e+05–1.784e+05) | 1.754e+05 (1.727e+05–1.82e+05) | 0.99× | 1.08× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 1.532e+05 (1.507e+05–1.577e+05) | 1.557e+05 (1.536e+05–1.678e+05) | 0.98× | 1.05× | within drift |
