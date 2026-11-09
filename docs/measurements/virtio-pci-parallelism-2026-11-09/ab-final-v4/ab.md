| Case | Workload | Unit | base | after | after vs base | ext4-guest-kernel after vs base | Verdict |
|---|---|---|---|---|---|---|---|
| fxfs-guest | scale-create-j1 | ops/s | 7.761e+04 (6.298e+04–8.533e+04) | 7.682e+04 (6.431e+04–8.606e+04) | 1.01× | 1.02× | within drift |
| fxfs-guest | scale-create-j2 | ops/s | 1.214e+05 (1.075e+05–1.316e+05) | 1.132e+05 (1.02e+05–1.329e+05) | 1.07× | 1.05× | within drift |
| fxfs-guest | scale-create-j4 | ops/s | 1.271e+05 (1.114e+05–1.318e+05) | 1.238e+05 (1.103e+05–1.38e+05) | 1.03× | 0.99× | within drift |
| fxfs-guest | scale-create-j8 | ops/s | 1.242e+05 (1.175e+05–1.331e+05) | 1.278e+05 (1.199e+05–1.441e+05) | 0.97× | 1.04× | within drift |
| fxfs-guest | scale-fsync-append-j1 | IOPS | 3,049 (2,783–3,413) | 3,568 (3,282–3,938) | 0.85× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j1-fsync-p50 | us | 286.7 (284.7–309.2) | 256 (238.6–264.2) | 0.89× | 0.98× | real |
| fxfs-guest | scale-fsync-append-j1-fsync-p99 | us | 693.2 (436.2–1,581) | 562.2 (399.4–1,696) | 0.81× | 0.97× | within drift |
| fxfs-guest | scale-fsync-append-j1-p50 | us | 3.04 (2.8–3.184) | 3.056 (2.704–3.376) | 1.01× | 1.05× | within drift |
| fxfs-guest | scale-fsync-append-j1-p99 | us | 10.05 (6.816–17.02) | 12.16 (7.392–14.53) | 1.21× | 0.84× | within drift |
| fxfs-guest | scale-fsync-append-j2 | IOPS | 3,612 (3,325–3,850) | 4,096 (3,657–4,433) | 0.88× | 1.09× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p50 | us | 491.5 (481.3–501.8) | 446.5 (423.9–456.7) | 0.91× | 1.18× | within drift |
| fxfs-guest | scale-fsync-append-j2-fsync-p99 | us | 2,212 (1,188–2,343) | 1,245 (823.3–2,310) | 0.56× | 0.93× | within drift |
| fxfs-guest | scale-fsync-append-j2-p50 | us | 3.936 (3.696–4.016) | 3.888 (3.6–4.128) | 0.99× | 2.43× | within drift |
| fxfs-guest | scale-fsync-append-j2-p99 | us | 16.86 (11.58–24.19) | 12.03 (10.43–21.38) | 0.71× | 0.99× | within drift |
| fxfs-guest | scale-fsync-append-j4 | IOPS | 5,114 (4,900–5,278) | 5,404 (5,306–5,535) | 0.95× | 0.96× | real |
| fxfs-guest | scale-fsync-append-j4-fsync-p50 | us | 602.1 (577.5–610.3) | 569.3 (553–585.7) | 0.95× | 0.96× | within drift |
| fxfs-guest | scale-fsync-append-j4-fsync-p99 | us | 3,686 (2,867–4,751) | 3,424 (3,162–4,686) | 0.93× | 0.95× | within drift |
| fxfs-guest | scale-fsync-append-j4-p50 | us | 4.224 (3.856–4.512) | 4.352 (4.128–4.64) | 1.03× | 1.00× | within drift |
| fxfs-guest | scale-fsync-append-j4-p99 | us | 41.98 (37.63–60.16) | 33.79 (31.36–40.19) | 0.80× | 0.95× | within drift |
| fxfs-guest | scale-fsync-append-j8 | IOPS | 5,584 (5,231–5,785) | 5,860 (5,588–6,032) | 0.95× | 0.95× | within drift |
| fxfs-guest | scale-fsync-append-j8-fsync-p50 | us | 790.5 (757.8–872.4) | 704.5 (643.1–716.8) | 0.89× | 0.96× | real |
| fxfs-guest | scale-fsync-append-j8-fsync-p99 | us | 9,503 (8,585–1.094e+04) | 1.022e+04 (8,585–1.265e+04) | 1.08× | 0.98× | within drift |
| fxfs-guest | scale-fsync-append-j8-p50 | us | 4.32 (4.08–4.512) | 4.416 (3.984–4.512) | 1.02× | 0.96× | within drift |
| fxfs-guest | scale-fsync-append-j8-p99 | us | 105.5 (92.67–119.3) | 82.94 (70.14–92.67) | 0.79× | 0.95× | within drift |
| fxfs-guest | scale-rand-read-cold-j1 | IOPS | 3,694 (3,562–3,772) | 3,875 (3,765–3,961) | 0.95× | 1.03× | within drift |
| fxfs-guest | scale-rand-read-cold-j1-p50 | us | 120.3 (114.2–122.4) | 108 (101.9–113.2) | 0.90× | 1.05× | real |
| fxfs-guest | scale-rand-read-cold-j1-p99 | us | 4,751 (4,620–4,948) | 4,719 (4,489–5,014) | 0.99× | 1.09× | within drift |
| fxfs-guest | scale-rand-read-cold-j2 | IOPS | 6,907 (6,628–7,288) | 7,258 (6,978–7,670) | 0.95× | 0.89× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p50 | us | 125.4 (123.4–128.5) | 112.6 (110.1–114.2) | 0.90× | 0.88× | within drift |
| fxfs-guest | scale-rand-read-cold-j2-p99 | us | 5,243 (5,014–5,603) | 5,145 (4,948–5,407) | 0.98× | 1.02× | within drift |
| fxfs-guest | scale-rand-read-cold-j4 | IOPS | 1.239e+04 (1.192e+04–1.276e+04) | 1.318e+04 (1.264e+04–1.374e+04) | 0.94× | 1.07× | within drift |
| fxfs-guest | scale-rand-read-cold-j4-p50 | us | 155.6 (152.6–162.8) | 136.2 (130.6–140.3) | 0.88× | 1.07× | real |
| fxfs-guest | scale-rand-read-cold-j4-p99 | us | 5,177 (4,620–5,407) | 5,341 (2,966–5,603) | 1.03× | 0.91× | within drift |
| fxfs-guest | scale-rand-read-cold-j8 | IOPS | 1.875e+04 (1.808e+04–1.979e+04) | 2.01e+04 (1.916e+04–2.09e+04) | 0.93× | 0.91× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p50 | us | 200.7 (195.6–207.9) | 186.4 (181.2–195.6) | 0.93× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-cold-j8-p99 | us | 6,947 (2,802–8,159) | 4,940 (2,638–7,176) | 0.71× | 0.68× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16 | IOPS | 3.175e+04 (2.968e+04–3.303e+04) | 2.768e+04 (2.592e+04–2.926e+04) | 1.15× | 1.43× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p50 | us | 466.9 (448.5–510) | 512 (501.8–544.8) | 1.10× | 1.43× | within drift |
| fxfs-guest | scale-rand-read-cold-qd16-p99 | us | 1,090 (1,036–1,221) | 1,368 (1,237–1,499) | 1.26× | 1.44× | within drift |
| fxfs-guest | scale-rand-read-warm-j1 | IOPS | 9.747e+05 (9.563e+05–9.85e+05) | 9.712e+05 (9.55e+05–9.866e+05) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p50 | us | 0.724 (0.716–0.748) | 0.732 (0.716–0.748) | 1.01× | 0.98× | within drift |
| fxfs-guest | scale-rand-read-warm-j1-p99 | us | 0.992 (0.98–1.004) | 0.992 (0.972–1.004) | 1.00× | 0.99× | within drift |
| fxfs-guest | scale-rand-read-warm-j2 | IOPS | 1.858e+06 (1.842e+06–1.871e+06) | 1.862e+06 (1.855e+06–1.868e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p50 | us | 0.772 (0.764–0.78) | 0.772 (0.772–0.78) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j2-p99 | us | 1.072 (1.048–1.112) | 1.048 (1.048–1.064) | 0.98× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4 | IOPS | 3.654e+06 (3.638e+06–3.678e+06) | 3.654e+06 (3.608e+06–3.665e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p50 | us | 0.788 (0.788–0.796) | 0.796 (0.788–0.796) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j4-p99 | us | 1.088 (1.08–1.24) | 1.096 (1.08–1.16) | 1.01× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8 | IOPS | 3.543e+06 (3.526e+06–3.561e+06) | 3.542e+06 (3.534e+06–3.551e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p50 | us | 0.82 (0.812–0.82) | 0.82 (0.82–0.82) | 1.00× | 1.00× | within drift |
| fxfs-guest | scale-rand-read-warm-j8-p99 | us | 1.176 (1.16–1.24) | 1.192 (1.16–1.208) | 1.01× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j1 | IOPS | 6.827e+04 (6.4e+04–6.827e+04) | 6.613e+04 (5.689e+04–6.827e+04) | 1.03× | 1.01× | within drift |
| fxfs-guest | scale-rand-write-j1-p50 | us | 1.368 (1.352–1.416) | 1.408 (1.368–1.432) | 1.03× | 1.03× | within drift |
| fxfs-guest | scale-rand-write-j1-p99 | us | 2.48 (2.16–2.864) | 2.448 (2.224–3.12) | 0.99× | 1.56× | within drift |
| fxfs-guest | scale-rand-write-j2 | IOPS | 1.024e+05 (9.752e+04–1.138e+05) | 1.051e+05 (4.995e+04–1.138e+05) | 0.97× | 0.98× | within drift |
| fxfs-guest | scale-rand-write-j2-p50 | us | 1.784 (1.752–1.832) | 1.768 (1.688–2.256) | 0.99× | 0.99× | within drift |
| fxfs-guest | scale-rand-write-j2-p99 | us | 6.624 (4.448–7.904) | 7.744 (4.512–12.35) | 1.17× | 1.48× | within drift |
| fxfs-guest | scale-rand-write-j4 | IOPS | 1.525e+05 (1.365e+05–1.638e+05) | 1.438e+05 (1.365e+05–1.517e+05) | 1.06× | 1.00× | within drift |
| fxfs-guest | scale-rand-write-j4-p50 | us | 2.028 (1.816–2.32) | 2.16 (2.096–2.192) | 1.07× | 0.98× | within drift |
| fxfs-guest | scale-rand-write-j4-p99 | us | 16.86 (7.008–27.26) | 23.42 (20.61–26.24) | 1.39× | 1.02× | within drift |
| fxfs-guest | scale-rand-write-j8 | IOPS | 1.518e+05 (1.343e+05–1.575e+05) | 1.321e+05 (9.87e+04–1.575e+05) | 1.15× | 1.04× | within drift |
| fxfs-guest | scale-rand-write-j8-p50 | us | 1.92 (1.832–2.256) | 1.944 (1.848–2.224) | 1.01× | 0.91× | within drift |
| fxfs-guest | scale-rand-write-j8-p99 | us | 21.38 (10.82–81.41) | 22.4 (10.56–71.17) | 1.05× | 1.03× | within drift |
| fxfs-guest | scale-stat-cold-j1 | ops/s | 1.488e+05 (1.165e+05–1.64e+05) | 1.625e+05 (1.267e+05–1.644e+05) | 0.92× | 1.10× | within drift |
| fxfs-guest | scale-stat-cold-j2 | ops/s | 2.183e+05 (1.88e+05–2.428e+05) | 2.161e+05 (1.702e+05–2.401e+05) | 1.01× | 1.07× | within drift |
| fxfs-guest | scale-stat-cold-j4 | ops/s | 2.926e+05 (2.803e+05–3.255e+05) | 3.107e+05 (2.866e+05–3.187e+05) | 0.94× | 0.92× | within drift |
| fxfs-guest | scale-stat-cold-j8 | ops/s | 2.785e+05 (2.238e+05–2.974e+05) | 2.843e+05 (2.759e+05–2.872e+05) | 0.98× | 0.95× | within drift |
| fxfs-guest | scale-unlink-j1 | ops/s | 1.951e+05 (1.799e+05–2.096e+05) | 2.352e+05 (2.166e+05–2.45e+05) | 0.83× | 0.97× | real |
| fxfs-guest | scale-unlink-j2 | ops/s | 2.016e+05 (1.525e+05–2.382e+05) | 2.013e+05 (1.701e+05–2.143e+05) | 1.00× | 1.04× | within drift |
| fxfs-guest | scale-unlink-j4 | ops/s | 2.078e+05 (1.837e+05–2.295e+05) | 2.177e+05 (2.097e+05–2.257e+05) | 0.95× | 0.90× | within drift |
| fxfs-guest | scale-unlink-j8 | ops/s | 1.969e+05 (1.818e+05–2.268e+05) | 2.056e+05 (2.015e+05–2.104e+05) | 0.96× | 0.96× | within drift |
| fxfs-guest-dax | scale-create-j1 | ops/s | 8.089e+04 (6.788e+04–8.538e+04) | 7.769e+04 (5.76e+04–8.753e+04) | 1.04× | 1.02× | within drift |
| fxfs-guest-dax | scale-create-j2 | ops/s | 1.224e+05 (1.083e+05–1.334e+05) | 1.189e+05 (1.001e+05–1.299e+05) | 1.03× | 1.05× | within drift |
| fxfs-guest-dax | scale-create-j4 | ops/s | 1.304e+05 (1.247e+05–1.339e+05) | 1.391e+05 (1.329e+05–1.472e+05) | 0.94× | 0.99× | within drift |
| fxfs-guest-dax | scale-create-j8 | ops/s | 1.259e+05 (1.158e+05–1.295e+05) | 1.324e+05 (1.269e+05–1.378e+05) | 0.95× | 1.04× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1 | IOPS | 3,059 (2,522–3,261) | 3,170 (2,709–3,413) | 0.96× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p50 | us | 294.9 (292.9–305.2) | 266.2 (250.9–272.4) | 0.90× | 0.98× | real |
| fxfs-guest-dax | scale-fsync-append-j1-fsync-p99 | us | 1,031 (477.2–2,540) | 1,466 (798.7–2,179) | 1.42× | 0.97× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p50 | us | 3.2 (2.96–3.344) | 3.264 (3.056–3.376) | 1.02× | 1.05× | within drift |
| fxfs-guest-dax | scale-fsync-append-j1-p99 | us | 13.25 (5.536–20.1) | 18.3 (9.024–24.7) | 1.38× | 0.84× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2 | IOPS | 3,474 (3,325–3,683) | 3,733 (3,495–3,923) | 0.93× | 1.09× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p50 | us | 503.8 (497.7–514) | 469 (460.8–481.3) | 0.93× | 1.18× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-fsync-p99 | us | 2,490 (1,810–2,605) | 2,044 (1,434–2,572) | 0.82× | 0.93× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p50 | us | 4.192 (3.984–4.448) | 4.256 (3.888–4.512) | 1.02× | 2.43× | within drift |
| fxfs-guest-dax | scale-fsync-append-j2-p99 | us | 28.67 (25.22–35.07) | 28.93 (18.82–33.54) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4 | IOPS | 5,159 (5,069–5,211) | 5,088 (5,032–5,211) | 1.01× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p50 | us | 598 (593.9–618.5) | 589.8 (577.5–602.1) | 0.99× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-fsync-p99 | us | 2,802 (2,474–2,900) | 3,244 (2,703–3,391) | 1.16× | 0.95× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p50 | us | 4.256 (4.128–4.384) | 4.672 (4.08–5.024) | 1.10× | 1.00× | within drift |
| fxfs-guest-dax | scale-fsync-append-j4-p99 | us | 41.73 (39.17–50.43) | 35.33 (29.57–38.14) | 0.85× | 0.95× | real |
| fxfs-guest-dax | scale-fsync-append-j8 | IOPS | 5,412 (5,265–5,603) | 5,393 (4,536–5,565) | 1.00× | 0.95× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p50 | us | 815.1 (798.7–831.5) | 757.8 (700.4–782.3) | 0.93× | 0.96× | real |
| fxfs-guest-dax | scale-fsync-append-j8-fsync-p99 | us | 8,176 (7,635–1.016e+04) | 1.016e+04 (8,716–1.488e+04) | 1.24× | 0.98× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p50 | us | 4.192 (4.048–4.32) | 4.448 (4.256–4.64) | 1.06× | 0.96× | within drift |
| fxfs-guest-dax | scale-fsync-append-j8-p99 | us | 116.7 (96.77–132.1) | 88.58 (76.29–106) | 0.76× | 0.95× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1 | IOPS | 5,299 (5,198–5,418) | 5,300 (4,911–5,835) | 1.00× | 1.03× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p50 | us | 13.89 (13.76–16.19) | 13.95 (13.63–14.27) | 1.00× | 1.05× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j1-p99 | us | 4,686 (4,620–4,817) | 4,751 (4,620–4,948) | 1.01× | 1.09× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2 | IOPS | 1.039e+04 (9,894–1.122e+04) | 1.117e+04 (1.027e+04–1.194e+04) | 0.93× | 0.89× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p50 | us | 12.61 (12.22–12.74) | 12.48 (12.1–12.74) | 0.99× | 0.88× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j2-p99 | us | 4,882 (3,949–5,014) | 4,850 (3,817–5,145) | 0.99× | 1.02× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4 | IOPS | 2.238e+04 (2.196e+04–2.643e+04) | 2.327e+04 (2.117e+04–2.513e+04) | 0.96× | 1.07× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p50 | us | 12.8 (12.61–13.12) | 12.67 (12.22–12.99) | 0.99× | 1.07× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j4-p99 | us | 4,325 (1,860–4,555) | 4,137 (2,073–4,751) | 0.96× | 0.91× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8 | IOPS | 3.736e+04 (3.539e+04–3.819e+04) | 3.767e+04 (3.378e+04–3.938e+04) | 0.99× | 0.91× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p50 | us | 13.06 (12.74–13.38) | 13.06 (12.86–13.38) | 1.00× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-j8-p99 | us | 2,310 (2,146–5,210) | 3,678 (2,146–5,407) | 1.59× | 0.68× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16 | IOPS | 6.827e+05 (6.827e+05–6.827e+05) | 6.827e+05 (5.12e+05–6.827e+05) | 1.00× | 1.43× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p50 | us | 15.36 (15.17–19.84) | 15.42 (15.3–20.35) | 1.00× | 1.43× | within drift |
| fxfs-guest-dax | scale-rand-read-cold-qd16-p99 | us | 50.18 (45.82–69.12) | 68.1 (54.53–79.36) | 1.36× | 1.44× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1 | IOPS | 9.104e+05 (8.951e+05–9.155e+05) | 9.025e+05 (8.918e+05–9.072e+05) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p50 | us | 0.796 (0.788–0.82) | 0.808 (0.804–0.828) | 1.02× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j1-p99 | us | 1.026 (1.02–1.032) | 1.032 (1.02–1.048) | 1.01× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2 | IOPS | 1.465e+06 (1.459e+06–1.48e+06) | 1.459e+06 (1.439e+06–1.467e+06) | 1.00× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p50 | us | 1.032 (1.012–1.048) | 1.04 (1.032–1.064) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j2-p99 | us | 1.464 (1.464–1.496) | 1.496 (1.464–1.512) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4 | IOPS | 2.529e+06 (2.461e+06–2.613e+06) | 2.503e+06 (2.461e+06–2.539e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p50 | us | 1.16 (1.144–1.192) | 1.184 (1.16–1.208) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j4-p99 | us | 2.192 (2.04–2.288) | 2.224 (2.192–2.288) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8 | IOPS | 2.529e+06 (2.509e+06–2.574e+06) | 2.493e+06 (2.437e+06–2.529e+06) | 1.01× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p50 | us | 1.168 (1.16–1.176) | 1.192 (1.176–1.208) | 1.02× | 1.00× | within drift |
| fxfs-guest-dax | scale-rand-read-warm-j8-p99 | us | 2.176 (2.064–2.224) | 2.256 (2.192–2.32) | 1.04× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j1 | IOPS | 6,361 (6,024–6,737) | 6,776 (6,024–7,314) | 0.94× | 1.01× | within drift |
| fxfs-guest-dax | scale-rand-write-j1-p50 | us | 140.3 (127.5–146.4) | 129.8 (117.2–150.5) | 0.93× | 1.03× | within drift |
| fxfs-guest-dax | scale-rand-write-j1-p99 | us | 205.8 (189.4–216.1) | 186.4 (177.2–201.7) | 0.91× | 1.56× | within drift |
| fxfs-guest-dax | scale-rand-write-j2 | IOPS | 1.191e+04 (1.157e+04–1.264e+04) | 1.339e+04 (1.191e+04–1.393e+04) | 0.89× | 0.98× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p50 | us | 111.1 (71.17–129.5) | 101.9 (91.65–110.1) | 0.92× | 0.99× | within drift |
| fxfs-guest-dax | scale-rand-write-j2-p99 | us | 264.2 (232.4–284.7) | 221.2 (218.1–246.8) | 0.84× | 1.48× | within drift |
| fxfs-guest-dax | scale-rand-write-j4 | IOPS | 1.074e+04 (1.032e+04–1.084e+04) | 1.191e+04 (1.184e+04–1.223e+04) | 0.90× | 1.00× | real |
| fxfs-guest-dax | scale-rand-write-j4-p50 | us | 317.4 (317.4–337.9) | 282.6 (268.3–284.7) | 0.89× | 0.98× | real |
| fxfs-guest-dax | scale-rand-write-j4-p99 | us | 569.3 (561.2–585.7) | 553 (536.6–553) | 0.97× | 1.02× | real |
| fxfs-guest-dax | scale-rand-write-j8 | IOPS | 1.038e+04 (1.027e+04–1.072e+04) | 1.067e+04 (1.038e+04–1.082e+04) | 0.97× | 1.04× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p50 | us | 716.8 (675.8–725) | 688.1 (684–692.2) | 0.96× | 0.91× | within drift |
| fxfs-guest-dax | scale-rand-write-j8-p99 | us | 1,114 (1,073–1,139) | 1,139 (1,122–1,303) | 1.02× | 1.03× | within drift |
| fxfs-guest-dax | scale-stat-cold-j1 | ops/s | 1.374e+05 (1.151e+05–1.559e+05) | 1.337e+05 (1.168e+05–1.585e+05) | 1.03× | 1.10× | within drift |
| fxfs-guest-dax | scale-stat-cold-j2 | ops/s | 2.153e+05 (1.748e+05–2.359e+05) | 2.03e+05 (6.015e+04–2.323e+05) | 1.06× | 1.07× | within drift |
| fxfs-guest-dax | scale-stat-cold-j4 | ops/s | 3.122e+05 (2.967e+05–3.314e+05) | 2.908e+05 (2.785e+05–3.392e+05) | 1.07× | 0.92× | within drift |
| fxfs-guest-dax | scale-stat-cold-j8 | ops/s | 2.82e+05 (2.729e+05–2.98e+05) | 2.843e+05 (2.812e+05–2.886e+05) | 0.99× | 0.95× | within drift |
| fxfs-guest-dax | scale-unlink-j1 | ops/s | 1.7e+05 (1.568e+05–2.078e+05) | 1.961e+05 (1.639e+05–2.093e+05) | 0.87× | 0.97× | within drift |
| fxfs-guest-dax | scale-unlink-j2 | ops/s | 2.138e+05 (2.027e+05–2.344e+05) | 1.98e+05 (1.875e+05–2.305e+05) | 1.08× | 1.04× | within drift |
| fxfs-guest-dax | scale-unlink-j4 | ops/s | 2.082e+05 (1.862e+05–2.201e+05) | 2.16e+05 (1.935e+05–2.222e+05) | 0.96× | 0.90× | within drift |
| fxfs-guest-dax | scale-unlink-j8 | ops/s | 1.995e+05 (1.747e+05–2.079e+05) | 2.033e+05 (1.972e+05–2.085e+05) | 0.98× | 0.96× | within drift |
