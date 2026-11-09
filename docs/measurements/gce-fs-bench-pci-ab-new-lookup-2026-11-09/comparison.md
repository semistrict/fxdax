Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 5.

| Workload | Unit | ext4-guest-kernel |
|---|---|---|
| create | ops/s | 71,388 (35,038–97,136) |
| stat-cold | ops/s | 334,142 (172,754–336,055) |
| stat-warm | ops/s | 842,792 (760,250–1,439,186) |
| lookup-missing | ops/s | 408,219 (327,199–437,681) |
| readdir | ops/s | 2,930,188 (986,317–3,043,593) |
| read-small-cold | ops/s | 23,081 (22,651–26,804) |
| chmod | ops/s | 724,730 (492,084–768,424) |
| setxattr | ops/s | 559,371 (436,854–617,402) |
| link | ops/s | 154,299 (144,109–156,433) |
| rename | ops/s | 119,264 (112,917–120,048) |
| rename-across | ops/s | 103,742 (101,776–104,011) |
| unlink | ops/s | 413,201 (370,218–424,889) |
| create-fsync | ops/s | 396 (360–422) |
| mkdir-tree | ops/s | 116,537 (110,463–131,499) |
| find-cold | ops/s | 321,812 (223,476–327,468) |
| rm-tree | ops/s | 106,884 (106,359–113,173) |
| large-dir-create | ops/s | 111,183 (79,217–112,504) |
| large-dir-stat-cold | ops/s | 261,246 (134,261–285,662) |
| large-dir-unlink | ops/s | 211,138 (207,260–214,166) |
| create-parallel | ops/s | 172,584 (168,554–178,730) |
| stat-parallel | ops/s | 593,499 (548,072–624,844) |
| unlink-parallel | ops/s | 290,814 (286,534–306,882) |
| create-shared-dir | ops/s | 110,726 (108,140–112,356) |
| unlink-shared-dir | ops/s | 279,376 (267,238–288,426) |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.01 median (-5.08–-2.87)

