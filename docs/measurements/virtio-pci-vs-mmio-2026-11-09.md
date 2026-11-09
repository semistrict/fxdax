# Firecracker's devices on virtio-PCI against virtio-MMIO (TASK-267)

TASK-267 attached every device of fxdax's VMs over virtio-PCI, the extent device's DAX window a BAR
of its own, where they were all on virtio-MMIO. Here the two transports, measured on one GCE host,
the same guest kernel and the same source, the VM process told which by FXDAX_VIRTIO_TRANSPORT
(`guest/tests/test_cases/src/vm.rs`'s `Transport`, Firecracker's `--enable-pci`).

**Runs.** One c3-standard-8 (8 vCPUs, 32 GiB, Sapphire Rapids, us-east4-a, kernel 7.0.0-1011-gcp),
`fxdax-keep-c3-standard-8-us-east4-a-1`, every run MMIO first, then PCI, on it; fxdax at 68afd0eb5,
Firecracker at 87ddcbefa (the stack before its replay onto fxdax-v1.16.1-r1: the same code as
fxdax-pci-r1, 517d438c5), the guest kernel built there in release. Each side's results are in the
directory beside this file.

- `bench-mmio`, `bench-pci`: `scripts/bench-gce.sh run`, cases `bench-fs-ext4-guest-kernel`
  (ext4 on virtio-blk), `bench-fs-fxfs-guest`, `bench-fs-fxfs-guest-dax`, groups io, files and
  fsync, three iterations: medians, with the range.
- `exits-mmio`, `exits-pci`: the same driver, `bench-fs-fxfs-guest(-dax)`, the fsync group alone
  (create-fsync), one iteration, with `FXDAX_BENCH_PERF=stat:kvm:kvm_exit,kvm:kvm_userspace_exit,kvm:kvm_mmio,kvm:kvm_msi_set_irq`:
  perf counting the VM process's KVM tracepoints each 100 ms, and the guest's Fxfs counting its
  device requests and their time.
- `boot-mmio`, `boot-pci`: `scripts/test-gce.sh run`, `extent-guest` and
  `fxfs-guest-snapshot-dax`, ten rounds each: the guest's boot, as every guest now says it on its
  console (`fxdax: boot kernel_ns=K agent_ns=A`, its kernel's start to running init and to the
  agent's), and the snapshot's restores (`/snapshot/load`, two a round).

## A request

Through the extent device, a write and a commit take 40% less over PCI: the guest's own count of
its device requests' time, create-fsync's 400 writes and 200 commits.

| | MMIO | PCI |
|---|---|---|
| fxfs-guest: a write | 121 µs | 73 µs |
| fxfs-guest: a commit | 126 µs | 75 µs |
| fxfs-guest-dax: a write | 109 µs | 71 µs |
| fxfs-guest-dax: a commit | 114 µs | 74 µs |

Over the VM's whole run (its boot and create-fsync, about 4 s), KVM's MMIO exits are a third of
MMIO's, and its exits all told the same: a timer's, a halt's and the boot's are most of them.
Over PCI, the device's interrupts are MSIs KVM injects (about 530), where over MMIO the guest's
handler reads the interrupt's status from the device and acknowledges it, two MMIO exits each
(TASK-91.15). Less the boot's (about 200 in the first 100 ms of either), about 4.6 MMIO exits a
request over MMIO and 1.3 over PCI, of create-fsync's 600.

| fxfs-guest / fxfs-guest-dax | MMIO | PCI |
|---|---|---|
| kvm_mmio | 2,989 / 2,930 | 987 / 995 |
| kvm_msi_set_irq | 0 / 0 | 529 / 527 |
| kvm_userspace_exit | 2,282 / 2,259 | 2,429 / 2,512 |
| kvm_exit | 41,401 / 41,972 | 42,478 / 42,344 |

## The workloads

What the device serves a request at a time is faster over PCI; what the guest's caches or the DAX
window serve is the same. Every workload, of the three cases, is in `bench-*/comparison.md`; those
that differ by more than their runs did:

| Workload | Case | MMIO | PCI | |
|---|---|---|---|---|
| rand-read-cold (IOPS) | ext4-guest-kernel | 14,385 | 30,118 | 2.09× |
| rand-read-cold (IOPS) | fxfs-guest | 5,074 | 5,777 | 1.14× |
| read-small-cold (ops/s) | ext4-guest-kernel | 15,298 | 24,125 | 1.58× |
| read-small-cold (ops/s) | fxfs-guest | 6,672 | 8,354 | 1.25× |
| sqlite-pread-scan (MiB/s) | ext4-guest-kernel | 1,710 | 2,771 | 1.62× |
| sqlite-pread-scan (MiB/s) | fxfs-guest | 322 | 352 | 1.09× |
| fsync-append (IOPS) | fxfs-guest | 2,351 | 2,619 | 1.11× |
| fsync-append (IOPS) | fxfs-guest-dax | 2,938 | 3,631 | 1.24× |
| create-fsync (ops/s) | fxfs-guest | 2,163 | 2,830 | 1.31× |
| create-fsync (ops/s) | fxfs-guest-dax | 2,488 | 2,847 | 1.14× |
| rand-write (IOPS) | fxfs-guest-dax | 5,500 | 6,302 | 1.15× |

No workload was slower over PCI by more than its runs varied: the warm reads, the mapped reads and
writes, and the metadata workloads but those that sync are within a few percent either way.
fxfs-guest's unlink, 179,104 against 328,762, is left out: MMIO's three runs spread from 171,906
to 359,848.

## Boot and restore

Neither costs more over PCI. The kernel's PCI enumeration is not to be seen in its boot, and a
restore, which maps the window's mappings again and activates the devices, takes as long.

| | MMIO | PCI |
|---|---|---|
| boot, the kernel's start to init (`extent-guest`, 10) | 271 ms (251-287) | 269 ms (259-291) |
| boot, to the guest agent (10) | 291 ms | 289 ms |
| a restore (`fxfs-guest-snapshot-dax`, 20) | 25.6 ms (22.0-1,913) | 26.1 ms (24.4-31.4) |

MMIO's slowest restores, 1.9 s, were both of one round's, the two at once; the other eighteen took
22-28 ms, the median theirs too.
`scripts/fork-time-gce.sh` times forks of a store in the extent daemon, which no VM takes part in,
so the transport changes nothing it measures; a VM's fork is its restore from a snapshot, above.

## The suites on PCI

Every guest suite passes over PCI: GCE pcirel5 (release guest kernel) 15 of 16, the one failure
TASK-257's flaky `a_removed_fill_gives_back_all_but_the_reservation` (fxfs-guest-checks-dax, the
filesystem's free space after a fill removed); pcidbg4 (debug guest kernel) 16 of 16; and on the
stack as landed, fxdax-pci-r1, the eleven cases that cover the PCI paths (extent-guest, fxfs-guest,
fxfs-guest-dax, the snapshots, zygotes, suspends, volumes, live fork, follow and daemon-lost) and
the vmm crate's suite, 771 of 772 (TASK-271's test_set_tsc, upstream's, red on every GCE host).

## MMIO's removal

PCI is as fast or faster in everything measured, and every suite passes on it, so it is the
default on x86-64 now. fxdax's own MMIO code can go, but not yet:

- aarch64 guests are on MMIO still: nothing here has run one over PCI (no arm64 KVM host), which
  needs its GICv3's ITS for MSI-X (TASK-268);
- a snapshot keeps its transport: one taken of an MMIO VM restores over MMIO alone, so MMIO stays
  as long as such snapshots are to be restored;
- what would go is fxdax's: the extent device's window past the MMIO gap (`window::Place::PastMmio64`),
  the MMIO transport's SHM registers (behind the fxdax feature in the series),
  FXDAX_VIRTIO_TRANSPORT=mmio and the MMIO save and restore of extent devices. Firecracker's own
  MMIO transport stays, upstream's and its default, which a build without the fxdax feature keeps
  as it is.

So: remove fxdax's MMIO paths once TASK-268 has aarch64 on PCI and no MMIO snapshot is kept.

TASK-273 (2026-11-09) removed them without waiting on TASK-268: with no deployments, no MMIO
snapshot is kept, and aarch64 went to PCI untested on KVM (TASK-268 runs it once an arm64 KVM host
exists).
