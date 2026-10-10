make: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cargo build --release --features blk --features net --features fxdax
cp target/release/libkrun.so target/release/libkrun.so.1.18.0
mkdir -p test-prefix
PREFIX="$(realpath test-prefix)" make install
make[1]: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
rm -f libkrun.pc libkrun.pc-t
sed -e 's|@prefix@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix|' \
    -e 's|@libdir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64|' \
    -e 's|@includedir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include|' \
    -e 's|@PACKAGE_NAME@|libkrun|' \
    -e 's|@PACKAGE_VERSION@|1.18.0|' \
    libkrun.pc.in > libkrun.pc-t
mv libkrun.pc-t libkrun.pc
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_display.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_input.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 libkrun.pc /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -m 755 target/release/libkrun.so.1.18.0 /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
cd /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/ ; ln -sf libkrun.so.1.18.0 libkrun.so.1 ; ln -sf libkrun.so.1 libkrun.so
make[1]: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cd tests; RUST_LOG=trace LIBKRUN_LIB_PATH="$(realpath ../test-prefix/lib64/):/var/lib/fxdax-test/libkrunfw/lib/x86_64-linux-gnu" PKG_CONFIG_PATH="$(realpath ../test-prefix/lib64/pkgconfig/)" ./run.sh test --test-case "bench-fs-ext4" --base-dir /home/ramon/run-20261010T165206Z/results/logs/bench-fs-ext4 --keep-all

running 1 test
test tests::all_testcases_have_unique_names ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

Downloading FreeBSD kernel...
FreeBSD kernel: target/freebsd-kernel/freebsd-kern.bin
FreeBSD sysroot or init/init-freebsd not found; FreeBSD tests will be skipped.
(Run 'make' with BUILD_BSD_INIT=1 in the libkrun root to build FreeBSD assets.)
Downloading gvproxy to target/gvproxy...
gvproxy: /home/ramon/run-20261010T165206Z/source/vendor/libkrun/tests/target/gvproxy
make: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
make: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cargo build --release --features blk --features net --features fxdax
cp target/release/libkrun.so target/release/libkrun.so.1.18.0
mkdir -p test-prefix
PREFIX="$(realpath test-prefix)" make install
make[1]: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
rm -f libkrun.pc libkrun.pc-t
sed -e 's|@prefix@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix|' \
    -e 's|@libdir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64|' \
    -e 's|@includedir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include|' \
    -e 's|@PACKAGE_NAME@|libkrun|' \
    -e 's|@PACKAGE_VERSION@|1.18.0|' \
    libkrun.pc.in > libkrun.pc-t
mv libkrun.pc-t libkrun.pc
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_display.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_input.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 libkrun.pc /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -m 755 target/release/libkrun.so.1.18.0 /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
cd /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/ ; ln -sf libkrun.so.1.18.0 libkrun.so.1 ; ln -sf libkrun.so.1 libkrun.so
make[1]: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cd tests; RUST_LOG=trace LIBKRUN_LIB_PATH="$(realpath ../test-prefix/lib64/):/var/lib/fxdax-test/libkrunfw/lib/x86_64-linux-gnu" PKG_CONFIG_PATH="$(realpath ../test-prefix/lib64/pkgconfig/)" ./run.sh test --test-case "bench-fs-fxdax-dax" --base-dir /home/ramon/run-20261010T165206Z/results/logs/bench-fs-fxdax-dax --keep-all

running 1 test
test tests::all_testcases_have_unique_names ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

FreeBSD kernel: target/freebsd-kernel/freebsd-kern.bin
FreeBSD sysroot or init/init-freebsd not found; FreeBSD tests will be skipped.
(Run 'make' with BUILD_BSD_INIT=1 in the libkrun root to build FreeBSD assets.)
gvproxy (cached): /home/ramon/run-20261010T165206Z/source/vendor/libkrun/tests/target/gvproxy
make: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
make: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cargo build --release --features blk --features net --features fxdax
cp target/release/libkrun.so target/release/libkrun.so.1.18.0
mkdir -p test-prefix
PREFIX="$(realpath test-prefix)" make install
make[1]: Entering directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
rm -f libkrun.pc libkrun.pc-t
sed -e 's|@prefix@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix|' \
    -e 's|@libdir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64|' \
    -e 's|@includedir@|/home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include|' \
    -e 's|@PACKAGE_NAME@|libkrun|' \
    -e 's|@PACKAGE_VERSION@|1.18.0|' \
    libkrun.pc.in > libkrun.pc-t
mv libkrun.pc-t libkrun.pc
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -d /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_display.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 include/libkrun_input.h /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/include
install -m 644 libkrun.pc /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/pkgconfig
install -m 755 target/release/libkrun.so.1.18.0 /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/
cd /home/ramon/run-20261010T165206Z/source/vendor/libkrun/test-prefix/lib64/ ; ln -sf libkrun.so.1.18.0 libkrun.so.1 ; ln -sf libkrun.so.1 libkrun.so
make[1]: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
cd tests; RUST_LOG=trace LIBKRUN_LIB_PATH="$(realpath ../test-prefix/lib64/):/var/lib/fxdax-test/libkrunfw/lib/x86_64-linux-gnu" PKG_CONFIG_PATH="$(realpath ../test-prefix/lib64/pkgconfig/)" ./run.sh test --test-case "bench-fs-fxdax-nodax" --base-dir /home/ramon/run-20261010T165206Z/results/logs/bench-fs-fxdax-nodax --keep-all

running 1 test
test tests::all_testcases_have_unique_names ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

FreeBSD kernel: target/freebsd-kernel/freebsd-kern.bin
FreeBSD sysroot or init/init-freebsd not found; FreeBSD tests will be skipped.
(Run 'make' with BUILD_BSD_INIT=1 in the libkrun root to build FreeBSD assets.)
gvproxy (cached): /home/ramon/run-20261010T165206Z/source/vendor/libkrun/tests/target/gvproxy
make: Leaving directory '/home/ramon/run-20261010T165206Z/source/vendor/libkrun'
Host: linux. Iterations: ext4-virtio-blk 1, fxdax-dax 1, fxdax-nodax 1.

| Workload | Unit | ext4-virtio-blk | fxdax-dax | fxdax-nodax | fxdax-dax vs ext4 | fxdax-nodax vs ext4 |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 104 (104–104) | 42.9 (42.9–42.9) | 74.0 (74.0–74.0) | **2.42×** | 1.41× |
| rand-read-cold | IOPS | 13,485 (13,485–13,485) | 11,728 (11,728–11,728) | 7,670 (7,670–7,670) | 1.15× | 1.76× |
| seq-read-cold | MiB/s | 641 (641–641) | 175 (175–175) | 511 (511–511) | **3.66×** | 1.25× |
| seq-read-warm | MiB/s | 2,709 (2,709–2,709) | 10,039 (10,039–10,039) | 1,620 (1,620–1,620) | 0.27× | 1.67× |
| rand-read-warm | IOPS | 9,610 (9,610–9,610) | 910,998 (910,998–910,998) | 8,798 (8,798–8,798) | 0.01× | 1.09× |
| rand-write | IOPS | 7,596 (7,596–7,596) | 5,636 (5,636–5,636) | 5,620 (5,620–5,620) | 1.35× | 1.35× |
| mmap-rand-read | IOPS | 61,419 (61,419–61,419) | 12,635 (12,635–12,635) | 9,471 (9,471–9,471) | **4.86×** | **6.49×** |
| mmap-rand-write | IOPS | 4,476 (4,476–4,476) | 1,740 (1,740–1,740) | 5,640 (5,640–5,640) | **2.57×** | 0.79× |
| fsync-append | IOPS | 320 (320–320) | 949 (949–949) | 950 (950–950) | 0.34× | 0.34× |
| create | ops/s | 62,710 (62,710–62,710) | 1,099 (1,099–1,099) | 1,078 (1,078–1,078) | **57.08×** | **58.17×** |
| stat-cold | ops/s | 233,526 (233,526–233,526) | 1,018 (1,018–1,018) | 999 (999–999) | **229.32×** | **233.74×** |
| readdir | ops/s | 3,012,053 (3,012,053–3,012,053) | 425,039 (425,039–425,039) | 405,524 (405,524–405,524) | **7.09×** | **7.43×** |
| unlink | ops/s | 78,483 (78,483–78,483) | 2,272 (2,272–2,272) | 2,271 (2,271–2,271) | **34.54×** | **34.56×** |
| untar | s | 0.10 (0.10–0.10) | 0.62 (0.62–0.62) | 1.00 (1.00–1.00) | **6.31×** | **10.22×** |
| configure | s | 37.0 (37.0–37.0) | 40.9 (40.9–40.9) | 41.7 (41.7–41.7) | 1.11× | 1.13× |
| make | s | 9.64 (9.64–9.64) | 10.5 (10.5–10.5) | 10.5 (10.5–10.5) | 1.09× | 1.09× |
| build-total | s | 46.7 (46.7–46.7) | 52.1 (52.1–52.1) | 53.2 (53.2–53.2) | 1.11× | 1.14× |

Over 2× slower than ext4:
- seq-write on fxdax-dax: 2.42×
- seq-read-cold on fxdax-dax: 3.66×
- mmap-rand-read on fxdax-dax: 4.86×
- mmap-rand-read on fxdax-nodax: 6.49×
- mmap-rand-write on fxdax-dax: 2.57×
- create on fxdax-dax: 57.08×
- create on fxdax-nodax: 58.17×
- stat-cold on fxdax-dax: 229.32×
- stat-cold on fxdax-nodax: 233.74×
- readdir on fxdax-dax: 7.09×
- readdir on fxdax-nodax: 7.43×
- unlink on fxdax-dax: 34.54×
- unlink on fxdax-nodax: 34.56×
- untar on fxdax-dax: 6.31×
- untar on fxdax-nodax: 10.22×
