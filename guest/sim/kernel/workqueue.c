// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Workqueues. A queued work is a task, which the scheduler runs among the others. Waiters for works
 * poll, sleeping a virtual millisecond at a time: nothing in dax.c depends on how soon they wake.
 */

#include <sim/sim.h>
#include <sim/host.h>

/* A queued work's run: on `running` from when it begins, as the work itself may go with it. */
struct sim_workqueue_run {
	struct list_head list;
	struct work_struct *work;
	struct workqueue_struct *wq;
};

static LIST_HEAD(running);

static bool work_running(const struct work_struct *work)
{
	struct sim_workqueue_run *run;

	list_for_each_entry(run, &running, list) {
		if (run->work == work)
			return true;
	}
	return false;
}

struct workqueue_struct *sim_alloc_workqueue(const char *name)
{
	struct workqueue_struct *wq = kzalloc(sizeof(*wq), GFP_KERNEL);

	wq->name = name;
	return wq;
}

static void wait_a_moment(const char *what)
{
	current->blocked_on = what;
	set_current_state(TASK_UNINTERRUPTIBLE);
	schedule_timeout(1);
	current->blocked_on = NULL;
}

static void work_task(void *arg)
{
	struct sim_workqueue_run *run = arg;
	struct work_struct *work = run->work;
	struct workqueue_struct *wq = run->wq;

	/* A work never runs beside itself: it queued again as it ran, and this run waits for it. */
	while (work_running(work))
		wait_a_moment("the same work to end");
	/* Not cancelled meanwhile. */
	if (READ_ONCE(work->pending)) {
		work->pending = false;
		list_add(&run->list, &running);
		work->func(work);
		list_del(&run->list);
	}
	kfree(run);
	wq->active--;
}

bool queue_work(struct workqueue_struct *wq, struct work_struct *work)
{
	struct sim_workqueue_run *run;

	if (work->pending)
		return false;
	work->pending = true;
	work->wq = wq;
	wq->active++;
	run = kmalloc(sizeof(*run), GFP_ATOMIC);
	run->work = work;
	run->wq = wq;
	sim_spawn(wq->name, work_task, run);
	return true;
}

bool flush_work(struct work_struct *work)
{
	bool waited = false;

	while (READ_ONCE(work->pending) || work_running(work)) {
		wait_a_moment("a work to end");
		waited = true;
	}
	return waited;
}

bool cancel_work_sync(struct work_struct *work)
{
	bool pending = READ_ONCE(work->pending);

	WRITE_ONCE(work->pending, false);
	while (work_running(work))
		wait_a_moment("a work to end");
	return pending;
}

void flush_workqueue(struct workqueue_struct *wq)
{
	while (READ_ONCE(wq->active))
		wait_a_moment("a workqueue to drain");
}

void drain_workqueue(struct workqueue_struct *wq)
{
	flush_workqueue(wq);
}

void destroy_workqueue(struct workqueue_struct *wq)
{
	flush_workqueue(wq);
	kfree(wq);
}
