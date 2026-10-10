First full run, 5 iterations, ext4 and fxdax: 512 MiB data, Fedora root filesystem, GNU make's
configure and build. fio dropped each job's cache before it ran, so its "warm" reads were cold.
Superseded by 2026-10-10T1823Z.
