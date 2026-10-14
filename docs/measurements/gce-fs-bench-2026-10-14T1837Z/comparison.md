Host: linux. Iterations: ext4-virtio-blk 3, ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-virtio-blk | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | ext4-guest-kernel vs ext4-virtio-blk | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 107 (96.4–119) | 107 (96.2–107) | 107 (106–119) | 88.4 (88.1–88.4) | 1.00× | 1.00× | 1.21× |
| rand-read-cold | IOPS | 13,299 (12,800–14,222) | 13,885 (9,752–14,372) | 3,477 (3,371–3,549) | 3,253 (3,251–3,259) | 0.96× | **3.99×** | **4.27×** |
| seq-read-cold | MiB/s | 2,943 (2,876–3,160) | 3,048 (2,485–3,122) | 163 (150–201) | 205 (191–211) | 0.97× | **18.74×** | **14.87×** |
| seq-read-warm | MiB/s | 7,606 (7,496–7,882) | 7,975 (7,675–8,014) | 8,051 (7,939–8,144) | 11,470 (11,322–11,731) | 0.95× | 0.99× | 0.70× |
| rand-read-warm | IOPS | 861,735 (847,991–863,804) | 870,354 (869,610–881,509) | 874,914 (864,074–879,448) | 859,768 (856,960–862,886) | 0.99× | 0.99× | 1.01× |
| rand-write | IOPS | 6,375 (6,361–10,415) | 6,382 (6,375–10,347) | 2,164 (2,159–2,164) | 1,550 (1,549–1,711) | 1.00× | **2.95×** | **4.12×** |
| mmap-rand-read | IOPS | 1,233,327 (1,226,182–1,235,526) | 1,246,178 (1,242,732–1,251,861) | 1,236,143 (1,232,960–1,254,529) | 1,154,032 (650,316–1,154,166) | 0.99× | 1.01× | 1.08× |
| mmap-rand-write | IOPS | 6,385 (6,382–6,385) | 6,385 (6,382–6,389) | 2,164 (2,157–2,165) | 1,550 (1,415–1,550) | 1.00× | **2.95×** | **4.12×** |
| fsync-append | IOPS | 287 (224–287) | 288 (288–288) | 281 (220–284) | 219 (218–224) | 1.00× | 1.02× | 1.31× |
| create | ops/s | 75,639 (68,197–76,085) | 77,836 (68,429–78,293) | 41,940 (39,633–51,058) | 908 (866–970) | 0.97× | 1.86× | **85.73×** |
| stat-cold | ops/s | 350,993 (193,600–354,870) | 366,108 (247,676–368,965) | 115,155 (106,446–125,109) | 135,979 (132,230–148,833) | 0.96× | **3.18×** | **2.69×** |
| stat-warm | ops/s | 1,364,452 (1,332,174–1,401,601) | 1,415,718 (1,283,032–1,467,003) | 1,545,897 (1,465,765–1,546,447) | 1,563,617 (1,332,621–1,596,845) | 0.96× | 0.92× | 0.91× |
| lookup-missing | ops/s | 490,218 (451,664–494,174) | 491,514 (483,486–498,105) | 425,065 (378,245–427,828) | 402,089 (317,350–427,172) | 1.00× | 1.16× | 1.22× |
| readdir | ops/s | 2,702,542 (2,656,967–2,914,959) | 2,637,030 (2,550,591–2,842,173) | 1,486,795 (1,359,303–1,524,828) | 1,373,698 (1,220,072–1,376,744) | 1.02× | 1.77× | 1.92× |
| read-small-cold | ops/s | 13,255 (13,233–13,427) | 13,450 (13,318–13,484) | 3,130 (3,090–3,866) | 2,183 (2,154–2,800) | 0.99× | **4.30×** | **6.16×** |
| chmod | ops/s | 797,462 (773,289–813,109) | 794,197 (784,986–821,569) | 101,252 (100,441–108,895) | 99,426 (84,415–124,463) | 1.00× | **7.84×** | **7.99×** |
| setxattr | ops/s | 616,014 (610,252–629,135) | 580,663 (574,866–583,352) | 149,841 (72,576–159,034) | 160,189 (153,369–167,269) | 1.06× | **3.88×** | **3.62×** |
| link | ops/s | 114,096 (113,802–115,640) | 117,302 (115,446–118,228) | 42,661 (39,046–49,251) | 45,822 (35,731–46,753) | 0.97× | **2.75×** | **2.56×** |
| rename | ops/s | 77,237 (77,043–77,399) | 77,271 (76,967–77,329) | 25,246 (23,650–27,766) | 23,676 (20,193–26,592) | 1.00× | **3.06×** | **3.26×** |
| rename-across | ops/s | 67,969 (67,798–68,044) | 69,559 (69,414–69,766) | 19,287 (19,204–23,363) | 21,986 (16,310–23,748) | 0.98× | **3.61×** | **3.16×** |
| unlink | ops/s | 206,359 (203,981–207,449) | 206,031 (204,068–206,703) | 38,855 (32,676–40,404) | 35,357 (28,990–35,672) | 1.00× | **5.30×** | **5.83×** |
| create-fsync | ops/s | 417 (371–425) | 385 (374–399) | 275 (265–285) | 221 (218–231) | 1.08× | 1.40× | 1.74× |
| mkdir-tree | ops/s | 90,858 (90,359–92,929) | 93,328 (90,280–93,656) | 41,011 (36,157–48,960) | 941 (934–997) | 0.97× | **2.28×** | **99.16×** |
| find-cold | ops/s | 341,461 (338,354–344,383) | 336,403 (333,955–337,838) | 114,650 (108,849–116,825) | 110,251 (107,774–134,910) | 1.02× | **2.93×** | **3.05×** |
| rm-tree | ops/s | 61,447 (61,209–61,880) | 61,186 (60,831–61,267) | 20,912 (18,209–23,106) | 23,625 (23,449–25,203) | 1.00× | **2.93×** | **2.59×** |
| large-dir-create | ops/s | 78,585 (67,790–82,941) | 75,256 (67,436–75,898) | 35,262 (33,194–36,554) | 34,343 (34,098–34,546) | 1.04× | **2.13×** | **2.19×** |
| large-dir-stat-cold | ops/s | 266,069 (249,378–288,561) | 268,986 (247,005–271,204) | 106,227 (104,649–106,734) | 105,635 (104,175–107,970) | 0.99× | **2.53×** | **2.55×** |
| large-dir-unlink | ops/s | 126,803 (126,608–127,275) | 126,780 (126,043–127,075) | 23,875 (23,689–25,942) | 24,594 (22,752–24,599) | 1.00× | **5.31×** | **5.15×** |
| create-parallel | ops/s | 134,200 (132,202–134,267) | 125,097 (122,593–135,454) | 38,022 (30,701–44,586) | 35,909 (33,170–38,270) | 1.07× | **3.29×** | **3.48×** |
| stat-parallel | ops/s | 640,034 (634,075–648,754) | 634,063 (616,843–690,895) | 673,278 (593,681–697,872) | 628,167 (570,440–702,067) | 1.01× | 0.94× | 1.01× |
| unlink-parallel | ops/s | 197,068 (195,954–220,668) | 203,488 (188,637–208,492) | 26,816 (25,711–28,742) | 25,825 (21,692–28,989) | 0.97× | **7.59×** | **7.88×** |
| create-shared-dir | ops/s | 85,702 (85,453–86,537) | 86,621 (85,939–88,368) | 35,628 (25,856–36,837) | 32,244 (30,650–34,694) | 0.99× | **2.43×** | **2.69×** |
| unlink-shared-dir | ops/s | 164,141 (162,865–165,833) | 164,667 (160,613–171,721) | 20,416 (20,234–21,882) | 19,545 (19,233–20,406) | 1.00× | **8.07×** | **8.42×** |
| untar | s | 0.09 (0.09–0.09) | 0.09 (0.09–0.09) | 0.11 (0.10–0.11) | 0.18 (0.17–0.21) | 1.00× | 1.14× | 1.91× |
| make | s | 6.87 (6.81–6.96) | 7.04 (6.55–7.22) | 7.09 (7.03–7.16) | 7.29 (7.12–7.31) | 1.02× | 1.01× | 1.03× |
| build-total | s | 6.96 (6.90–7.05) | 7.14 (6.64–7.31) | 7.21 (7.13–7.27) | 7.46 (7.33–7.48) | 1.02× | 1.01× | 1.05× |

- ext4-virtio-blk: mtime-ahead-of-guest-ms -2.44 median (-2.90–-1.06)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.62 median (-5.54–-3.96)
- fxfs-guest: mtime-ahead-of-guest-ms -2.74 median (-2.79–-1.97)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -7.86 median (-12.43–-7.23)

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 3.99× ext4-guest-kernel's
- rand-read-cold on fxfs-guest-dax: 4.27× ext4-guest-kernel's
- seq-read-cold on fxfs-guest: 18.74× ext4-guest-kernel's
- seq-read-cold on fxfs-guest-dax: 14.87× ext4-guest-kernel's
- rand-write on fxfs-guest: 2.95× ext4-guest-kernel's
- rand-write on fxfs-guest-dax: 4.12× ext4-guest-kernel's
- mmap-rand-write on fxfs-guest: 2.95× ext4-guest-kernel's
- mmap-rand-write on fxfs-guest-dax: 4.12× ext4-guest-kernel's
- create on fxfs-guest-dax: 85.73× ext4-guest-kernel's
- stat-cold on fxfs-guest: 3.18× ext4-guest-kernel's
- stat-cold on fxfs-guest-dax: 2.69× ext4-guest-kernel's
- read-small-cold on fxfs-guest: 4.30× ext4-guest-kernel's
- read-small-cold on fxfs-guest-dax: 6.16× ext4-guest-kernel's
- chmod on fxfs-guest: 7.84× ext4-guest-kernel's
- chmod on fxfs-guest-dax: 7.99× ext4-guest-kernel's
- setxattr on fxfs-guest: 3.88× ext4-guest-kernel's
- setxattr on fxfs-guest-dax: 3.62× ext4-guest-kernel's
- link on fxfs-guest: 2.75× ext4-guest-kernel's
- link on fxfs-guest-dax: 2.56× ext4-guest-kernel's
- rename on fxfs-guest: 3.06× ext4-guest-kernel's
- rename on fxfs-guest-dax: 3.26× ext4-guest-kernel's
- rename-across on fxfs-guest: 3.61× ext4-guest-kernel's
- rename-across on fxfs-guest-dax: 3.16× ext4-guest-kernel's
- unlink on fxfs-guest: 5.30× ext4-guest-kernel's
- unlink on fxfs-guest-dax: 5.83× ext4-guest-kernel's
- mkdir-tree on fxfs-guest: 2.28× ext4-guest-kernel's
- mkdir-tree on fxfs-guest-dax: 99.16× ext4-guest-kernel's
- find-cold on fxfs-guest: 2.93× ext4-guest-kernel's
- find-cold on fxfs-guest-dax: 3.05× ext4-guest-kernel's
- rm-tree on fxfs-guest: 2.93× ext4-guest-kernel's
- rm-tree on fxfs-guest-dax: 2.59× ext4-guest-kernel's
- large-dir-create on fxfs-guest: 2.13× ext4-guest-kernel's
- large-dir-create on fxfs-guest-dax: 2.19× ext4-guest-kernel's
- large-dir-stat-cold on fxfs-guest: 2.53× ext4-guest-kernel's
- large-dir-stat-cold on fxfs-guest-dax: 2.55× ext4-guest-kernel's
- large-dir-unlink on fxfs-guest: 5.31× ext4-guest-kernel's
- large-dir-unlink on fxfs-guest-dax: 5.15× ext4-guest-kernel's
- create-parallel on fxfs-guest: 3.29× ext4-guest-kernel's
- create-parallel on fxfs-guest-dax: 3.48× ext4-guest-kernel's
- unlink-parallel on fxfs-guest: 7.59× ext4-guest-kernel's
- unlink-parallel on fxfs-guest-dax: 7.88× ext4-guest-kernel's
- create-shared-dir on fxfs-guest: 2.43× ext4-guest-kernel's
- create-shared-dir on fxfs-guest-dax: 2.69× ext4-guest-kernel's
- unlink-shared-dir on fxfs-guest: 8.07× ext4-guest-kernel's
- unlink-shared-dir on fxfs-guest-dax: 8.42× ext4-guest-kernel's
