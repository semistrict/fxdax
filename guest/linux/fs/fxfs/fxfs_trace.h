/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * fxdax: the guest Fxfs's tracepoints (TASK-44.16), events/fxfs/ in tracefs, for perf, bpftrace and
 * ftrace to see the DAX protocol's steps and measure what each takes: a _start and an _end of each
 * step that waits, or a begin and an end, the device's sharing an id. They are the device's
 * requests, READ, WRITE, COMMIT, FREE and the rest, as submitted and as completed; the window's
 * MAP (one, or a MAP_MANY's), UNMAP and PRIVATE, calls the guest waits for, and why the window maps
 * (fxfs_dax_map); seals; DAX faults; the window's reclaim; a file's writeback and rebind; and
 * fsync. An event off costs a static branch. docs/guest-tracepoints.md says how to use them.
 * device.c defines them (CREATE_TRACE_POINTS).
 *
 * The id is the guest's own, counted as an event that starts a request is on, 0 while it is off:
 * the device's requests carry none (doc-4's wire protocol), and extentd's log names a MAP by its
 * window offset, as fxfs_window_map_start does.
 *
 * The fields are of the UAPI's types (__u64, __u8 for a flag), which the events' formats name: a
 * guest kernel has no BTF, so bpftrace compiles the formats against <linux/types.h>, which a
 * rootfs has as the UAPI's headers alone (Alpine's linux-headers), without the kernel's u64 or
 * bool (fxfs-guest-bpftrace-dax).
 */
#undef TRACE_SYSTEM
#define TRACE_SYSTEM fxfs

#if !defined(_FXFS_TRACE_H) || defined(TRACE_HEADER_MULTI_READ)
#define _FXFS_TRACE_H

#include <linux/tracepoint.h>
#include <linux/virtio_extent.h>

#define fxfs_trace_request_types					\
	{ VIRTIO_EXTENT_T_READ, "read" },				\
	{ VIRTIO_EXTENT_T_WRITE, "write" },				\
	{ VIRTIO_EXTENT_T_FREE, "free" },				\
	{ VIRTIO_EXTENT_T_MAP, "map" },					\
	{ VIRTIO_EXTENT_T_UNMAP, "unmap" },				\
	{ VIRTIO_EXTENT_T_COMMIT, "commit" },				\
	{ VIRTIO_EXTENT_T_ROOT, "root" },				\
	{ VIRTIO_EXTENT_T_INFO, "info" },				\
	{ VIRTIO_EXTENT_T_RESIZED, "resized" },				\
	{ VIRTIO_EXTENT_T_FREEZE, "freeze" },				\
	{ VIRTIO_EXTENT_T_ADVANCE, "advance" },				\
	{ VIRTIO_EXTENT_T_LET_GO, "let_go" },				\
	{ VIRTIO_EXTENT_T_PIN, "pin" },					\
	{ VIRTIO_EXTENT_T_UNPIN, "unpin" },				\
	{ VIRTIO_EXTENT_T_PRIVATE, "private" },				\
	{ VIRTIO_EXTENT_T_STATS, "stats" }

/*
 * A device request Fxfs's Rust submitted (fxfs_device_submit), which it doesn't wait for here:
 * `len` bytes of data at `offset` in extent `branch`:`sequence`.
 */
TRACE_EVENT(fxfs_device_submit,
	TP_PROTO(u64 id, u32 type, u32 flags, u64 branch, u64 sequence, u64 offset, u64 len),
	TP_ARGS(id, type, flags, branch, sequence, offset, len),
	TP_STRUCT__entry(
		__field(__u64, id)
		__field(__u32, type)
		__field(__u32, flags)
		__field(__u64, branch)
		__field(__u64, sequence)
		__field(__u64, offset)
		__field(__u64, len)
	),
	TP_fast_assign(
		__entry->id = id;
		__entry->type = type;
		__entry->flags = flags;
		__entry->branch = branch;
		__entry->sequence = sequence;
		__entry->offset = offset;
		__entry->len = len;
	),
	TP_printk("id=%llu type=%s flags=0x%x extent=%llx:%llu offset=%llu len=%llu", __entry->id,
		  __print_symbolic(__entry->type, fxfs_trace_request_types), __entry->flags,
		  __entry->branch, __entry->sequence, __entry->offset, __entry->len)
);

/* That request completed, or failed to be submitted: 0, or a -errno. */
TRACE_EVENT(fxfs_device_done,
	TP_PROTO(u64 id, u32 type, int status),
	TP_ARGS(id, type, status),
	TP_STRUCT__entry(
		__field(__u64, id)
		__field(__u32, type)
		__field(int, status)
	),
	TP_fast_assign(
		__entry->id = id;
		__entry->type = type;
		__entry->status = status;
	),
	TP_printk("id=%llu type=%s status=%d", __entry->id,
		  __print_symbolic(__entry->type, fxfs_trace_request_types), __entry->status)
);

/*
 * A window MAP, read-only, of an extent's pages or of zeros (doc-13): `count` maps in one request,
 * a MAP_MANY's, the first's extent, offset and window offset given, `len` all of theirs.
 */
TRACE_EVENT(fxfs_window_map_start,
	TP_PROTO(u64 id, u64 branch, u64 sequence, u64 offset, u64 len, u64 window_offset,
		 unsigned int count, bool zeros),
	TP_ARGS(id, branch, sequence, offset, len, window_offset, count, zeros),
	TP_STRUCT__entry(
		__field(__u64, id)
		__field(__u64, branch)
		__field(__u64, sequence)
		__field(__u64, offset)
		__field(__u64, len)
		__field(__u64, window_offset)
		__field(unsigned int, count)
		__field(__u8, zeros)
	),
	TP_fast_assign(
		__entry->id = id;
		__entry->branch = branch;
		__entry->sequence = sequence;
		__entry->offset = offset;
		__entry->len = len;
		__entry->window_offset = window_offset;
		__entry->count = count;
		__entry->zeros = zeros;
	),
	TP_printk("id=%llu extent=%llx:%llu offset=%llu len=%llu window=0x%llx count=%u zeros=%d",
		  __entry->id, __entry->branch, __entry->sequence, __entry->offset, __entry->len,
		  __entry->window_offset, __entry->count, __entry->zeros)
);

/*
 * A window UNMAP, or a PRIVATE (a store's copy-on-write page, doc-13): `len` bytes at
 * `window_offset`.
 */
DECLARE_EVENT_CLASS(fxfs_window_range,
	TP_PROTO(u64 id, u64 window_offset, u64 len),
	TP_ARGS(id, window_offset, len),
	TP_STRUCT__entry(
		__field(__u64, id)
		__field(__u64, window_offset)
		__field(__u64, len)
	),
	TP_fast_assign(
		__entry->id = id;
		__entry->window_offset = window_offset;
		__entry->len = len;
	),
	TP_printk("id=%llu window=0x%llx len=%llu", __entry->id, __entry->window_offset,
		  __entry->len)
);
DEFINE_EVENT(fxfs_window_range, fxfs_window_unmap_start,
	TP_PROTO(u64 id, u64 window_offset, u64 len), TP_ARGS(id, window_offset, len));
DEFINE_EVENT(fxfs_window_range, fxfs_window_private_start,
	TP_PROTO(u64 id, u64 window_offset, u64 len), TP_ARGS(id, window_offset, len));

/* A window request's end: what it returned, 0 or a -errno. */
DECLARE_EVENT_CLASS(fxfs_window_end,
	TP_PROTO(u64 id, int err),
	TP_ARGS(id, err),
	TP_STRUCT__entry(
		__field(__u64, id)
		__field(int, err)
	),
	TP_fast_assign(
		__entry->id = id;
		__entry->err = err;
	),
	TP_printk("id=%llu err=%d", __entry->id, __entry->err)
);
DEFINE_EVENT(fxfs_window_end, fxfs_window_map_end,
	TP_PROTO(u64 id, int err), TP_ARGS(id, err));
DEFINE_EVENT(fxfs_window_end, fxfs_window_unmap_end,
	TP_PROTO(u64 id, int err), TP_ARGS(id, err));
DEFINE_EVENT(fxfs_window_end, fxfs_window_private_end,
	TP_PROTO(u64 id, int err), TP_ARGS(id, err));

#define FXFS_TRACE_MAP_REACH 0
#define FXFS_TRACE_MAP_RANGE 1
#define FXFS_TRACE_MAP_REBIND 2
#define FXFS_TRACE_MAP_ZEROS 3
#define fxfs_trace_map_whys						\
	{ FXFS_TRACE_MAP_REACH, "reach" },				\
	{ FXFS_TRACE_MAP_RANGE, "range" },				\
	{ FXFS_TRACE_MAP_REBIND, "rebind" },				\
	{ FXFS_TRACE_MAP_ZEROS, "zeros" }

/*
 * Why the window maps, just before it does: a region's extent as far as a slice needs (reach, of
 * an extent files share: `ino` 0), a range's own pages (range), a range's private pages rebound
 * to their sealed extent (rebind), or zeros past a file's new end (zeros). `count` maps, `len`
 * bytes all told, the first at `window_offset`.
 */
TRACE_EVENT(fxfs_dax_map,
	TP_PROTO(unsigned int why, u64 ino, u64 window_offset, u64 len, unsigned int count),
	TP_ARGS(why, ino, window_offset, len, count),
	TP_STRUCT__entry(
		__field(unsigned int, why)
		__field(__u64, ino)
		__field(__u64, window_offset)
		__field(__u64, len)
		__field(unsigned int, count)
	),
	TP_fast_assign(
		__entry->why = why;
		__entry->ino = ino;
		__entry->window_offset = window_offset;
		__entry->len = len;
		__entry->count = count;
	),
	TP_printk("why=%s ino=%llu window=0x%llx len=%llu count=%u",
		  __print_symbolic(__entry->why, fxfs_trace_map_whys), __entry->ino,
		  __entry->window_offset, __entry->len, __entry->count)
);

/*
 * A seal begun: `seals` the device's count after it, odd while one is in progress. Its request, a
 * COMMIT or a WRITE with a barrier, is among the device's events before its end.
 */
TRACE_EVENT(fxfs_seal_begin,
	TP_PROTO(u64 seals),
	TP_ARGS(seals),
	TP_STRUCT__entry(
		__field(__u64, seals)
	),
	TP_fast_assign(
		__entry->seals = seals;
	),
	TP_printk("seals=%llu", __entry->seals)
);

/* A seal ended: the count, even, and the bytes it sealed, written since the seal before. */
TRACE_EVENT(fxfs_seal_end,
	TP_PROTO(u64 seals, u64 bytes),
	TP_ARGS(seals, bytes),
	TP_STRUCT__entry(
		__field(__u64, seals)
		__field(__u64, bytes)
	),
	TP_fast_assign(
		__entry->seals = seals;
		__entry->bytes = bytes;
	),
	TP_printk("seals=%llu bytes=%llu", __entry->seals, __entry->bytes)
);

/* A fault of a DAX mapping: of `order` (0, a page; PMD_ORDER), storing if `write`. */
TRACE_EVENT(fxfs_dax_fault_start,
	TP_PROTO(u64 ino, u64 pgoff, unsigned int order, bool write),
	TP_ARGS(ino, pgoff, order, write),
	TP_STRUCT__entry(
		__field(__u64, ino)
		__field(__u64, pgoff)
		__field(unsigned int, order)
		__field(__u8, write)
	),
	TP_fast_assign(
		__entry->ino = ino;
		__entry->pgoff = pgoff;
		__entry->order = order;
		__entry->write = write;
	),
	TP_printk("ino=%llu pgoff=%llu order=%u write=%d", __entry->ino, __entry->pgoff,
		  __entry->order, __entry->write)
);

/* A fault's end: its vm_fault_t. */
TRACE_EVENT(fxfs_dax_fault_end,
	TP_PROTO(u64 ino, u64 pgoff, unsigned int ret),
	TP_ARGS(ino, pgoff, ret),
	TP_STRUCT__entry(
		__field(__u64, ino)
		__field(__u64, pgoff)
		__field(unsigned int, ret)
	),
	TP_fast_assign(
		__entry->ino = ino;
		__entry->pgoff = pgoff;
		__entry->ret = ret;
	),
	TP_printk("ino=%llu pgoff=%llu ret=0x%x", __entry->ino, __entry->pgoff, __entry->ret)
);

/* The window tried to reclaim a file's range, of chunk `chunk`, for room: whether it did. */
TRACE_EVENT(fxfs_dax_reclaim,
	TP_PROTO(u64 ino, u64 chunk, bool reclaimed),
	TP_ARGS(ino, chunk, reclaimed),
	TP_STRUCT__entry(
		__field(__u64, ino)
		__field(__u64, chunk)
		__field(__u8, reclaimed)
	),
	TP_fast_assign(
		__entry->ino = ino;
		__entry->chunk = chunk;
		__entry->reclaimed = reclaimed;
	),
	TP_printk("ino=%llu chunk=%llu reclaimed=%d", __entry->ino, __entry->chunk,
		  __entry->reclaimed)
);

/* A file's step that waits, by inode: its start. */
DECLARE_EVENT_CLASS(fxfs_inode_start,
	TP_PROTO(u64 ino),
	TP_ARGS(ino),
	TP_STRUCT__entry(
		__field(__u64, ino)
	),
	TP_fast_assign(
		__entry->ino = ino;
	),
	TP_printk("ino=%llu", __entry->ino)
);
/* Its end: a count (pages written back, or rebound) or 0, or a -errno, as the step returns. */
DECLARE_EVENT_CLASS(fxfs_inode_end,
	TP_PROTO(u64 ino, long ret),
	TP_ARGS(ino, ret),
	TP_STRUCT__entry(
		__field(__u64, ino)
		__field(long, ret)
	),
	TP_fast_assign(
		__entry->ino = ino;
		__entry->ret = ret;
	),
	TP_printk("ino=%llu ret=%ld", __entry->ino, __entry->ret)
);
DEFINE_EVENT(fxfs_inode_start, fxfs_dax_writeback_start, TP_PROTO(u64 ino), TP_ARGS(ino));
DEFINE_EVENT(fxfs_inode_end, fxfs_dax_writeback_end, TP_PROTO(u64 ino, long ret),
	TP_ARGS(ino, ret));
DEFINE_EVENT(fxfs_inode_start, fxfs_dax_rebind_start, TP_PROTO(u64 ino), TP_ARGS(ino));
DEFINE_EVENT(fxfs_inode_end, fxfs_dax_rebind_end, TP_PROTO(u64 ino, long ret),
	TP_ARGS(ino, ret));
DEFINE_EVENT(fxfs_inode_start, fxfs_fsync_start, TP_PROTO(u64 ino), TP_ARGS(ino));
DEFINE_EVENT(fxfs_inode_end, fxfs_fsync_end, TP_PROTO(u64 ino, long ret), TP_ARGS(ino, ret));

#endif /* _FXFS_TRACE_H */

#undef TRACE_INCLUDE_PATH
#define TRACE_INCLUDE_PATH .
#undef TRACE_INCLUDE_FILE
#define TRACE_INCLUDE_FILE fxfs_trace
#include <trace/define_trace.h>
