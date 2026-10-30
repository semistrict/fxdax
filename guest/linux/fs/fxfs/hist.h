/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * fxdax: latency histograms (TASK-111.6): how many of something took each power of two of time,
 * so that a run's tails and shifts show, not only its totals. Bucket k counts those under 1.024 us
 * times 2 to the k: the first, under 1.024 us; the last, the rest, from 4.3 s. The filesystem's
 * Rust counts its own the same (guest/rust/fxfs-linux's device module, Timed). Shown as the name
 * with _hist and the buckets' counts, comma-separated: one word, as every stat's value is.
 */
#ifndef _FS_FXFS_HIST_H
#define _FS_FXFS_HIST_H

#include <linux/atomic.h>
#include <linux/bitops.h>
#include <linux/minmax.h>
#include <linux/seq_file.h>

#define FXFS_HIST_BUCKETS 24

struct fxfs_hist {
	atomic64_t buckets[FXFS_HIST_BUCKETS];
};

/* Counts one that took `ns`. */
static inline void fxfs_hist_add(struct fxfs_hist *hist, u64 ns)
{
	atomic64_inc(&hist->buckets[min(fls64(ns >> 10), FXFS_HIST_BUCKETS - 1)]);
}

static inline void fxfs_hist_show(struct seq_file *m, const char *name, struct fxfs_hist *hist)
{
	int bucket;

	seq_printf(m, "%s_hist ", name);
	for (bucket = 0; bucket < FXFS_HIST_BUCKETS; bucket++)
		seq_printf(m, "%s%lld", bucket ? "," : "", atomic64_read(&hist->buckets[bucket]));
	seq_putc(m, '\n');
}

#endif
