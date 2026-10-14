// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * The kernel's side of the filesystem's Rust runtime (see rt.h).
 */

#include <linux/completion.h>
#include <linux/fxfs_tls.h>
#include <linux/hrtimer.h>
#include <linux/kernel.h>
#include <linux/ktime.h>
#include <linux/log2.h>
#include <linux/mm.h>
#include <linux/printk.h>
#include <linux/random.h>
#include <linux/sched/mm.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

#include "rt.h"

void *fxfs_rt_alloc(size_t size, size_t align)
{
	if (align > PAGE_SIZE)
		return NULL;
	/*
	 * kmalloc aligns sizes that are powers of two to themselves, and vmalloc, which kvmalloc
	 * falls back to, aligns to pages.
	 */
	if (align > ARCH_KMALLOC_MINALIGN)
		size = roundup_pow_of_two(max(size, align));
	return kvmalloc(size, GFP_KERNEL);
}

void fxfs_rt_free(void *ptr)
{
	kvfree(ptr);
}

void fxfs_rt_log(int level, const char *msg, size_t len)
{
	static const char *const levels[] = {
		KERN_EMERG, KERN_ALERT, KERN_CRIT, KERN_ERR,
		KERN_WARNING, KERN_NOTICE, KERN_INFO, KERN_DEBUG,
	};

	printk("%sfxfs: %.*s\n", levels[clamp(level, 0, 7)], (int)len, msg);
}

void fxfs_rt_panic(const char *msg, size_t len)
{
	panic("fxfs: %.*s\n", (int)len, msg);
}

struct fxfs_rt_parker {
	struct completion unparked;
};

struct fxfs_rt_parker *fxfs_rt_parker_new(void)
{
	struct fxfs_rt_parker *parker = kmalloc(sizeof(*parker), GFP_KERNEL);

	if (parker)
		init_completion(&parker->unparked);
	return parker;
}

void fxfs_rt_parker_free(struct fxfs_rt_parker *parker)
{
	kfree(parker);
}

void fxfs_rt_park(struct fxfs_rt_parker *parker)
{
	wait_for_completion(&parker->unparked);
}

void fxfs_rt_unpark(struct fxfs_rt_parker *parker)
{
	complete(&parker->unparked);
}

struct fxfs_rt_work {
	struct work_struct work;
	void (*run)(void *arg);
	void *arg;
};

static void fxfs_rt_work_run(struct work_struct *work)
{
	struct fxfs_rt_work *w = container_of(work, struct fxfs_rt_work, work);
	/* Rust's allocations must not recurse into filesystems to reclaim memory. */
	unsigned int nofs = memalloc_nofs_save();

	w->run(w->arg);
	memalloc_nofs_restore(nofs);
	kfree(w);
}

int fxfs_rt_schedule(void (*run)(void *arg), void *arg)
{
	struct fxfs_rt_work *w = kmalloc(sizeof(*w), GFP_ATOMIC);

	if (!w)
		return -ENOMEM;
	INIT_WORK(&w->work, fxfs_rt_work_run);
	w->run = run;
	w->arg = arg;
	queue_work(system_unbound_wq, &w->work);
	return 0;
}

struct fxfs_rt_timer {
	struct hrtimer timer;
	void (*fire)(void *arg);
	void *arg;
};

static enum hrtimer_restart fxfs_rt_timer_fire(struct hrtimer *timer)
{
	struct fxfs_rt_timer *t = container_of(timer, struct fxfs_rt_timer, timer);

	t->fire(t->arg);
	return HRTIMER_NORESTART;
}

struct fxfs_rt_timer *fxfs_rt_timer_new(void (*fire)(void *arg), void *arg)
{
	struct fxfs_rt_timer *t = kmalloc(sizeof(*t), GFP_KERNEL);

	if (!t)
		return NULL;
	hrtimer_init(&t->timer, CLOCK_MONOTONIC, HRTIMER_MODE_REL_SOFT);
	t->timer.function = fxfs_rt_timer_fire;
	t->fire = fire;
	t->arg = arg;
	return t;
}

void fxfs_rt_timer_start(struct fxfs_rt_timer *timer, u64 ns)
{
	hrtimer_start(&timer->timer, ns_to_ktime(ns), HRTIMER_MODE_REL_SOFT);
}

void fxfs_rt_timer_free(struct fxfs_rt_timer *timer)
{
	hrtimer_cancel(&timer->timer);
	kfree(timer);
}

u64 fxfs_rt_monotonic_ns(void)
{
	return ktime_get_ns();
}

u64 fxfs_rt_realtime_ns(void)
{
	return ktime_get_real_ns();
}

void fxfs_rt_random_bytes(void *buf, size_t len)
{
	get_random_bytes(buf, len);
}

/* Rust's, once the filesystem's Rust uses guest/rust/std, which has thread-local values. */
void __weak fxfs_rust_tls_free(void *tls)
{
}

void **fxfs_rt_tls(void)
{
	if (!in_task())
		return NULL;
	return &current->fxfs_tls;
}

void fxfs_rt_tls_exit(struct task_struct *tsk)
{
	void *tls = tsk->fxfs_tls;

	if (tls) {
		tsk->fxfs_tls = NULL;
		fxfs_rust_tls_free(tls);
	}
}
