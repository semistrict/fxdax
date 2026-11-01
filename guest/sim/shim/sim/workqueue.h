/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* Workqueues: each queued work runs in a simulated task of its own, unbound, as dax.c's do. */
#ifndef _SIM_WORKQUEUE_H
#define _SIM_WORKQUEUE_H

#include <sim/sched.h>

struct work_struct;
typedef void (*work_func_t)(struct work_struct *work);

struct work_struct {
	work_func_t func;
	struct workqueue_struct *wq;
	/* Queued, not yet begun: queue_work's bit. */
	bool pending;
};

struct workqueue_struct {
	const char *name;
	/* Works queued or running, which flushes wait for. */
	int active;
};

#define WQ_UNBOUND (1 << 1)
#define WQ_MEM_RECLAIM (1 << 3)

#define INIT_WORK(work, f) do {			\
	(work)->func = (f);			\
	(work)->wq = NULL;			\
	(work)->pending = false;		\
} while (0)

struct workqueue_struct *sim_alloc_workqueue(const char *name);
#define alloc_workqueue(name, flags, max_active, ...) sim_alloc_workqueue(name)
bool queue_work(struct workqueue_struct *wq, struct work_struct *work);
bool flush_work(struct work_struct *work);
bool cancel_work_sync(struct work_struct *work);
void flush_workqueue(struct workqueue_struct *wq);
void drain_workqueue(struct workqueue_struct *wq);
void destroy_workqueue(struct workqueue_struct *wq);

#endif
