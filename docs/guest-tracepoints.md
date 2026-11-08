# The guest Fxfs's tracepoints

For whoever measures or debugs the guest's Fxfs: the guest kernel's Fxfs has Linux tracepoints at
the steps of the DAX protocol (backlog doc-13) and at its device's requests (doc-4). This page lists
them and shows how to read them with tracefs, perf and bpftrace, with a script that measures how
long seals and window MAPs take.

The events are defined in `guest/linux/fs/fxfs/fxfs_trace.h`, and `device.c` creates them. They
sit beside two other tools:

- `/proc/fs/fxfs/<tag>`, the counters and latency histograms;
- the `fxfs.dax_trace` ring of a page's maps and iomaps, which tests dump to the kernel log.

A tracepoint that is off costs one static branch. The guest kernel has event tracing on
(`guest/linux/configs/fxdax.config`: `CONFIG_FTRACE`, `CONFIG_ENABLE_DEFAULT_TRACERS`, `CONFIG_BPF_EVENTS`), but no
function tracer. `scripts/build-guest-kernel.sh` checks `CONFIG_FTRACE` and
`CONFIG_BPF_EVENTS`.

## The events

All of them are in tracefs under `events/fxfs/`.

| Event | When | Fields |
|---|---|---|
| `fxfs_device_submit` | Fxfs's Rust submits a device request, which it does not wait for here | `id`, `type` (read, write, commit, free…), `flags`, `branch`, `sequence` (the extent), `offset`, `len` |
| `fxfs_device_done` | The request completes, or it could not be submitted | `id`, `type`, `status` (0 or -errno) |
| `fxfs_window_map_start` | A MAP of the window, read-only: an extent's pages, or zeros. A MAP_MANY counts as one, with `count` maps | `id`, `branch`, `sequence`, `offset` (the first map's), `len` (all of them), `window` (the first map's window offset), `count`, `zeros` |
| `fxfs_window_map_end` | The MAP returns | `id`, `err` |
| `fxfs_window_unmap_start`, `_end` | An UNMAP of `len` bytes at `window` | `id`, `window`, `len`; then `id`, `err` |
| `fxfs_window_private_start`, `_end` | A PRIVATE: a store's copy-on-write of window pages | `id`, `window`, `len`; then `id`, `err` |
| `fxfs_dax_map` | Why the window is about to map, just before the MAPs that follow on the same task | `why` (see below), `ino` (0 for `reach`), `window`, `len`, `count` |
| `fxfs_seal_begin` | A seal begins | `seals`: the device's count, odd while a seal is in progress |
| `fxfs_seal_end` | The seal ends | `seals` (one more), `bytes`: what WRITEs submitted since the last seal ended wrote, which this seal sealed |
| `fxfs_dax_fault_start` | A fault of a DAX mapping | `ino`, `pgoff`, `order` (0 is a page, 9 a PMD), `write` |
| `fxfs_dax_fault_end` | The fault returns | `ino`, `pgoff`, `ret` (its `vm_fault_t`) |
| `fxfs_dax_reclaim` | The window tried to reclaim a file's range for room | `ino`, `chunk`, `reclaimed` |
| `fxfs_dax_writeback_start`, `_end` | A DAX file's dirty pages are written back | `ino`; then `ino`, `ret` (pages written, or -errno) |
| `fxfs_dax_rebind_start`, `_end` | A file's clean private pages are mapped back to their sealed extents | `ino`; then `ino`, `ret` (pages rebound) |
| `fxfs_fsync_start`, `_end` | fsync(2) and fdatasync(2), of any file | `ino`; then `ino`, `ret` |

`why` in `fxfs_dax_map` takes one of four values:

- `reach` (0): a region's extent, mapped as far as a slice of it needs. Files share the region, so
  `ino` is 0.
- `range` (1): a range's own pages.
- `rebind` (2): a range's private pages, mapped back to their sealed extent.
- `zeros` (3): zeros past a file's new end.

### Matching a start to its end

A start and its end share an `id`. A device request's `submit` and `done` share one too; its
`done` can run on another CPU, from the device's interrupt.

The ids are the guest's own count. They are not on the wire: the device protocol's requests carry
no id, and these events do not change it. An id is 0 when the start event was off, for example for
a request that began before tracing did.

To match a guest MAP with the host's side, use its window offset. The host's daemon does not log
its MAPs yet (TASK-44.16, criterion 1).

## tracefs

In the guest, as root:

```sh
mount -t tracefs nodev /sys/kernel/tracing 2>/dev/null
cd /sys/kernel/tracing
echo 1 > events/fxfs/enable          # or events/fxfs/fxfs_seal_begin/enable, one at a time
echo > trace
# ... the workload ...
echo 0 > events/fxfs/enable
cat trace
```

A line looks like this:

```
guest-agent-412 [001] ..... 12.345678: fxfs_window_map_start: id=7 extent=1:42 offset=0 len=65536 window=0x200000 count=3 zeros=0
```

To see only some events, use a filter. For example, `echo 'err != 0' > events/fxfs/fxfs_window_map_end/filter`
keeps only the MAPs that failed.

## perf

```sh
perf list 'fxfs:*'
perf stat -e 'fxfs:fxfs_window_map_start,fxfs:fxfs_window_private_start,fxfs:fxfs_seal_begin' -a -- sleep 10
perf record -e 'fxfs:*' -a -- ./workload && perf script
```

`perf stat` counts the events. `perf record` with `perf script` prints them with their fields, as
tracefs does.

## bpftrace: seal and MAP latency

`scripts/bpftrace/fxfs-latency.bt` measures seals and window MAPs. It times each seal from
`fxfs_seal_begin` to the matching `fxfs_seal_end`, pairing them by their counts. It times each MAP
from `fxfs_window_map_start` to the `fxfs_window_map_end` with the same id. It groups the MAPs by the
`why` of the `fxfs_dax_map` before them on the same task.

```
tracepoint:fxfs:fxfs_seal_begin
{
	@seal_began[args.seals] = nsecs;
}

tracepoint:fxfs:fxfs_seal_end
/@seal_began[args.seals - 1]/
{
	@seal_us = hist((nsecs - @seal_began[args.seals - 1]) / 1000);
	@sealed_bytes = sum(args.bytes);
	delete(@seal_began[args.seals - 1]);
}

tracepoint:fxfs:fxfs_dax_map
{
	@why[tid] = args.why;
}

tracepoint:fxfs:fxfs_window_map_start
{
	@map_began[args.id] = nsecs;
	@map_why[args.id] = @why[tid];
	@map_bytes = hist(args.len);
}

tracepoint:fxfs:fxfs_window_map_end
/@map_began[args.id]/
{
	$why = @map_why[args.id];
	$us = (nsecs - @map_began[args.id]) / 1000;
	if ($why == 0) { @map_us["reach"] = hist($us); }
	else if ($why == 1) { @map_us["range"] = hist($us); }
	else if ($why == 2) { @map_us["rebind"] = hist($us); }
	else { @map_us["zeros"] = hist($us); }
	...
}
```

This is an excerpt; the file has the whole script. Run it in the guest, with bpftrace in its
rootfs, while the workload runs. The guest kernel has no BTF, so bpftrace compiles the events'
formats against `<linux/types.h>`: the rootfs needs the kernel's userspace (UAPI) headers too, such
as Alpine's `linux-headers`. The events' fields are of the UAPI's types (`__u64`, `__u8`) for
that reason.

```sh
bpftrace scripts/bpftrace/fxfs-latency.bt
# Ctrl-C prints @seal_us, @map_us[<why>] and @map_bytes as histograms, and @sealed_bytes
```

The guest test `fxfs-guest-bpftrace-dax` runs this script in a guest with Alpine's bpftrace. Its
workload writes and fsyncs files, reads them in sealed through a mapping, stores to them and fsyncs
again. The test checks that the script prints each histogram.

The guest has no BPF JIT (`CONFIG_BPF_JIT` is off), so bpftrace's programs run in the
interpreter, which adds to what each traced event costs. That cost is not measured.

## Tests

- **fxfs-guest-checks' `tracepoints` check** (`guest/tests/test_cases/src/test_fxfs_guest_checks/guest/tracepoints.rs`)
  turns the events on through tracefs. It writes a file and fsyncs it. It checks, in order: the
  fsync's start, a device WRITE completed with the same id, a seal's begin and end (counts n and
  n + 1), and the fsync's end, and that the seals sealed what was written. On a DAX mount it then
  checks a load, a store and another fsync, in order: the read fault, the `range` MAP of the file's
  pages, the write fault's PRIVATE of a window page within what that MAP mapped, the fsync's
  writeback, the seal, and each one's end. Across the whole trace, it checks that every start has
  one end with the same id.
- **fxfs-guest-bpftrace-dax** runs the script above in a guest.

## What it costs

Event tracing in the guest kernel costs boot time: on GCE (n2-standard-16, nested, release
kernel, 2026-11-08), the guest reached init in 0.294 s median over 5 boots (0.287 s at best),
against 0.251 s median over 15 boots (0.240 s at best) on main before it, on the same host: about
40–45 ms more. The kernel's size wasn't measured. An event that is off costs a static branch at
run time.
