// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The scheduler: tasks are coroutines, one running at a time, and each scheduling point asks a
 * generator seeded at the start which task runs next. Nothing else decides, not the host's clock
 * or threads, so a seed names a run: its interleaving, and so what it finds.
 *
 * Time is virtual. Each scheduling point moves the clock a little, and when every task sleeps it
 * moves to the first that wakes; a task waiting 10 ms costs nothing. If none can ever wake the
 * tasks are deadlocked, which is reported with what each holds and waits for.
 *
 * Every waiter is woken (TASK-149.18). A wait with no time limit that no wake comes to is a
 * deadlock, or a livelock if others go on; but one with a time limit, as dax.c's waits for room are,
 * ends anyway, a wake lost costing only time. So a timed wait's sleep that its time ends, its entry
 * queued still, ends early, as a spurious wake, a jiffy left: a wait that then finds its condition
 * true, its entry queued, was due a wake that never came, which fails the run (finish_wait). Its
 * condition false, the wait goes on, its time up. And no task ends with an entry queued, which a
 * wake would find on a stack gone.
 */

#include <sim/sim.h>
#include <sim/host.h>

/* The nanoseconds of virtual time a scheduling point takes. */
#define SIM_POINT_NS 100
/* Points before a run counts as a livelock: of tasks spinning, as TASK-140's write did. */
#define SIM_MAX_POINTS 20000000ULL

struct task_struct *sim_current;

static LIST_HEAD(tasks);
/*
 * Tasks done, whose stacks the next task to run frees, a coroutine can't free its own, and those
 * nothing holds, a work's, with them; the rest, which their spawners may look at, a run frees as it
 * ends (`freed`). Kept until a run ended, the workqueue's, a task each work queued, filled the
 * memory of a run whose works were queued again and again, as reclaim waiting for room has the
 * budget's worker (read_pressure, a sweep's failed UNMAP).
 */
static LIST_HEAD(ended);
static LIST_HEAD(freed);
static struct task_struct main_task = { .pid = 0, .comm = "main" };
static u64 rng_state;
/* In percent: how likely a point is to leave the current task running, which the seed picks. */
static unsigned int sticky;
static u64 now_ns, trace_hash, points, switches;
static bool tracing;
static int next_pid = 1;

/* Readers inside RCU, and the callbacks waiting for there to be none. */
static int rcu_readers;
static struct rcu_head *rcu_pending;

u64 sim_random(void)
{
	u64 x = rng_state;

	x ^= x << 13;
	x ^= x >> 7;
	x ^= x << 17;
	rng_state = x;
	return x;
}

static void trace_mix(u64 value)
{
	/* FNV-1a. */
	trace_hash = (trace_hash ^ value) * 0x100000001b3ULL;
}

void sim_init(u64 seed)
{
	static const unsigned int stickies[] = { 0, 50, 90, 98 };
	extern sim_initcall_t __start_sim_initcalls[], __stop_sim_initcalls[];
	sim_initcall_t *call;

	host_fixed_addresses();
	rng_state = seed * 0x9e3779b97f4a7c15ULL + 0x2545f4914f6cdd1dULL;
	sim_random();
	sticky = stickies[sim_random() % ARRAY_SIZE(stickies)];
	trace_hash = 0xcbf29ce484222325ULL;
	main_task.fiber = host_fiber_self();
	INIT_LIST_HEAD(&main_task.tasks);
	list_add_tail(&main_task.tasks, &tasks);
	sim_current = &main_task;
	for (call = __start_sim_initcalls; call < __stop_sim_initcalls; call++)
		(*call)();
}

u64 sim_trace_hash(void)
{
	return trace_hash;
}

u64 sim_points(void)
{
	return points;
}

u64 sim_switches(void)
{
	return switches;
}

void sim_trace_on(void)
{
	tracing = true;
}

u64 ktime_get_ns(void)
{
	return now_ns;
}

static void rcu_run_callbacks(void)
{
	struct rcu_head *head = rcu_pending;

	rcu_pending = NULL;
	while (head) {
		struct rcu_head *next = head->next;

		head->func(head);
		head = next;
	}
}

static void report_deadlock(void)
{
	struct task_struct *task;
	int i;

	sim_log("sim: DEADLOCK: no task can run, at %llu points\n", points);
	list_for_each_entry(task, &tasks, tasks) {
		if (task->done)
			continue;
		sim_log("sim:   %s (pid %d) waits for %s", task->comm, task->pid,
			task->blocked_on ?: "a wake");
		if (task->blocked_lock)
			sim_log(" %p", task->blocked_lock);
		sim_log(", holding");
		for (i = 0; i < task->nr_held; i++)
			sim_log(" %s (%s)", task->held[i].name, task->held[i].write ? "w" : "r");
		sim_log("%s\n", task->nr_held ? "" : " nothing");
	}
	sim_panic("deadlocked");
}

/* Wakes the sleepers whose time has come; if `idle`, moves the clock to the first of them. */
static bool wake_sleepers(bool idle)
{
	struct task_struct *task;
	u64 first = 0;
	bool woke = false;

	list_for_each_entry(task, &tasks, tasks) {
		if (task->wake_ns && task->__state != TASK_RUNNING &&
		    (!first || task->wake_ns < first))
			first = task->wake_ns;
	}
	if (!first)
		return false;
	if (idle && first > now_ns) {
		now_ns = first;
		sim_sweep_watch(now_ns);
	}
	list_for_each_entry(task, &tasks, tasks) {
		if (task->wake_ns && task->wake_ns <= now_ns && task->__state != TASK_RUNNING) {
			task->wake_ns = 0;
			task->__state = TASK_RUNNING;
			task->timed_out = true;
			woke = true;
		}
	}
	return woke;
}

/*
 * Picks the next task to run, among those that can, leaving this one running a share of the time
 * (`sticky`) unless `yield` asks: and switches to it.
 */
/* Frees the tasks done, from another. */
static void free_ended(void)
{
	struct task_struct *task, *next;

	list_for_each_entry_safe(task, next, &ended, tasks) {
		host_fiber_free(task->fiber);
		task->fiber = NULL;
		if (task->detached) {
			list_del(&task->tasks);
			host_free(task);
		} else {
			list_move(&task->tasks, &freed);
		}
	}
}

static void pick_next(const char *what, bool yield)
{
	struct task_struct *task, *next = NULL, *prev = current;
	unsigned int count = 0, at;

	if (!prev->done)
		free_ended();

	wake_sleepers(false);
	if (!rcu_readers && rcu_pending)
		rcu_run_callbacks();
	for (;;) {
		count = 0;
		list_for_each_entry(task, &tasks, tasks) {
			if (!task->done && task->__state == TASK_RUNNING)
				count++;
		}
		if (count)
			break;
		if (!wake_sleepers(true))
			report_deadlock();
	}
	if (prev->__state == TASK_RUNNING && !prev->done && (count == 1 ||
	    (!yield && sim_random() % 100 < sticky))) {
		next = prev;
	} else {
		at = sim_random() % count;
		list_for_each_entry(task, &tasks, tasks) {
			if (task->done || task->__state != TASK_RUNNING)
				continue;
			if (!at--) {
				next = task;
				break;
			}
		}
	}
	trace_mix(next->pid);
	trace_mix(points);
	if (tracing)
		sim_log("sim: %llu %s %s -> %s\n", points, prev->comm, what, next->comm);
	if (next == prev)
		return;
	switches++;
	sim_current = next;
	host_fiber_switch(prev->fiber, next->fiber);
}

void sim_point(const char *what)
{
	points++;
	now_ns += SIM_POINT_NS;
	if (points > SIM_MAX_POINTS)
		sim_panic("livelock: %llu scheduling points", points);
	pick_next(what, false);
}

void sim_might_sleep(const char *file, int line)
{
	if (current->atomic || current->rcu_depth)
		sim_warn(file, line, "%s sleeps in an atomic section (%d spinlocks, %d RCU)\n",
			 current->comm, current->atomic, current->rcu_depth);
}

void schedule(void)
{
	sim_might_sleep("schedule", 0);
	points++;
	now_ns += SIM_POINT_NS;
	pick_next("schedule", true);
	current->wake_ns = 0;
}

void cond_resched(void)
{
	sim_might_sleep("cond_resched", 0);
	sim_point("cond_resched");
}

/* Whether `task` has a wait queue entry queued: a wake would take it off. */
static bool queued(const struct task_struct *task)
{
	return task->waiting && !list_empty(&task->waiting->entry);
}

long schedule_timeout(long timeout)
{
	u64 deadline;

	if (timeout == MAX_SCHEDULE_TIMEOUT) {
		schedule();
		return timeout;
	}
	/* A timed wait's condition false after its early end: its time is up. */
	if (current->probed && queued(current)) {
		current->probed = false;
		__set_current_state(TASK_RUNNING);
		sim_point("a timed wait's end");
		return 0;
	}
	current->probed = false;
	current->timed_out = false;
	deadline = now_ns + timeout * (NSEC_PER_SEC / HZ);
	current->wake_ns = deadline;
	schedule();
	if (now_ns < deadline)
		return (deadline - now_ns) / (NSEC_PER_SEC / HZ) + 1;
	/* Ended by its time, its entry queued: early, for the wait to look at its condition. */
	if (current->timed_out && queued(current)) {
		current->probed = true;
		return 1;
	}
	return 0;
}

int wake_up_process(struct task_struct *task)
{
	if (task->__state == TASK_RUNNING)
		return 0;
	task->__state = TASK_RUNNING;
	task->timed_out = false;
	return 1;
}

void fsleep(unsigned long us)
{
	u64 deadline = now_ns + us * NSEC_PER_USEC;

	sim_might_sleep(__FILE__, __LINE__);
	set_current_state(TASK_UNINTERRUPTIBLE);
	current->wake_ns = deadline;
	current->blocked_on = "a sleep";
	schedule();
	current->blocked_on = NULL;
}

void msleep(unsigned int ms)
{
	fsleep(ms * 1000UL);
}

/* Each task's coroutine: its function, then the end of it. */
static void task_main(void *arg)
{
	struct task_struct *task = arg;

	task->fn(task->arg);
	if (task->nr_held)
		sim_panic("%s ended holding %s", task->comm, task->held[0].name);
	if (queued(task))
		sim_panic("%s ended with its wait queue entry queued", task->comm);
	task->done = true;
	task->__state = TASK_NORMAL;
	list_move(&task->tasks, &ended);
	wake_up_process(&main_task);
	pick_next("exit", true);
	sim_panic("%s ran after its end", task->comm);
}

void sim_spawn_detached(const char *name, void (*fn)(void *arg), void *arg)
{
	sim_spawn(name, fn, arg)->detached = true;
}

struct task_struct *sim_spawn(const char *name, void (*fn)(void *arg), void *arg)
{
	struct task_struct *task = host_malloc(sizeof(*task));

	memset(task, 0, sizeof(*task));
	task->pid = next_pid++;
	task->fault_drops = -1;
	task->comm = name;
	task->fn = fn;
	task->arg = arg;
	task->mm = current ? current->mm : NULL;
	task->fiber = host_fiber_new(task_main, task);
	list_add_tail(&task->tasks, &tasks);
	return task;
}

void sim_run(void)
{
	struct task_struct *task, *next;

	/* Only the main task left. */
	while (!list_is_singular(&tasks)) {
		main_task.__state = TASK_UNINTERRUPTIBLE;
		main_task.blocked_on = "the tasks to end";
		pick_next("sim_run", true);
		main_task.blocked_on = NULL;
	}
	if (rcu_pending)
		rcu_run_callbacks();
	free_ended();
	list_for_each_entry_safe(task, next, &freed, tasks) {
		list_del(&task->tasks);
		host_free(task);
	}
}

void rcu_read_lock(void)
{
	current->rcu_depth++;
	rcu_readers++;
}

void rcu_read_unlock(void)
{
	current->rcu_depth--;
	rcu_readers--;
}

void call_rcu(struct rcu_head *head, rcu_callback_t func)
{
	head->func = func;
	head->next = rcu_pending;
	rcu_pending = head;
}

void synchronize_rcu(void)
{
	while (rcu_readers)
		cond_resched();
}

/* Wait queues, as kernel/sched/wait.c's. */

void __init_waitqueue_head(struct wait_queue_head *wq_head, const char *name,
			   struct lock_class_key *key)
{
	sim_spin_init(&wq_head->lock, name);
	INIT_LIST_HEAD(&wq_head->head);
}

int default_wake_function(struct wait_queue_entry *wq_entry, unsigned int mode, int flags,
			  void *key)
{
	struct task_struct *task = wq_entry->private;

	/* Woken, if late: running already, its time having ended its sleep, its entry queued. */
	if (task->waiting == wq_entry) {
		task->timed_out = false;
		task->probed = false;
	}
	if (!(task->__state & mode))
		return 0;
	return wake_up_process(task);
}

int autoremove_wake_function(struct wait_queue_entry *wq_entry, unsigned int mode, int sync,
			     void *key)
{
	int ret = default_wake_function(wq_entry, mode, sync, key);

	if (ret)
		list_del_init_careful(&wq_entry->entry);
	return ret;
}

/* That `wq_entry`, of the current task's, is queued now. */
static void queue_entry(struct wait_queue_entry *wq_entry)
{
	if (wq_entry->private == current)
		current->waiting = wq_entry;
}

int __wake_up(struct wait_queue_head *wq_head, unsigned int mode, int nr, void *key)
{
	struct wait_queue_entry *curr, *next;

	spin_lock(&wq_head->lock);
	list_for_each_entry_safe(curr, next, &wq_head->head, entry) {
		unsigned int flags = curr->flags;
		int ret = curr->func(curr, mode, 0, key);

		if (ret < 0)
			break;
		if (ret && (flags & WQ_FLAG_EXCLUSIVE) && !--nr)
			break;
	}
	spin_unlock(&wq_head->lock);
	return 0;
}

void init_wait_entry(struct wait_queue_entry *wq_entry, int flags)
{
	wq_entry->flags = flags;
	wq_entry->private = current;
	wq_entry->func = autoremove_wake_function;
	INIT_LIST_HEAD(&wq_entry->entry);
}

static void prepare(struct wait_queue_head *wq_head, struct wait_queue_entry *wq_entry, int state,
		    bool exclusive)
{
	spin_lock(&wq_head->lock);
	if (list_empty(&wq_entry->entry)) {
		if (exclusive) {
			wq_entry->flags |= WQ_FLAG_EXCLUSIVE;
			list_add_tail(&wq_entry->entry, &wq_head->head);
		} else {
			wq_entry->flags &= ~WQ_FLAG_EXCLUSIVE;
			list_add(&wq_entry->entry, &wq_head->head);
		}
		queue_entry(wq_entry);
	}
	set_current_state(state);
	spin_unlock(&wq_head->lock);
}

void prepare_to_wait(struct wait_queue_head *wq_head, struct wait_queue_entry *wq_entry, int state)
{
	prepare(wq_head, wq_entry, state, false);
}

bool prepare_to_wait_exclusive(struct wait_queue_head *wq_head, struct wait_queue_entry *wq_entry,
			       int state)
{
	prepare(wq_head, wq_entry, state, true);
	return false;
}

long prepare_to_wait_event(struct wait_queue_head *wq_head, struct wait_queue_entry *wq_entry,
			   int state)
{
	if (signal_pending_state(state, current)) {
		spin_lock(&wq_head->lock);
		list_del_init(&wq_entry->entry);
		spin_unlock(&wq_head->lock);
		return -ERESTARTSYS;
	}
	prepare(wq_head, wq_entry, state, wq_entry->flags & WQ_FLAG_EXCLUSIVE);
	return 0;
}

/*
 * As the kernel's; and a timed wait that ends after its sleep ended early (schedule_timeout), its
 * entry queued, ends as its condition is true, which nothing woke it for: a wake lost.
 */
void finish_wait(struct wait_queue_head *wq_head, struct wait_queue_entry *wq_entry)
{
	__set_current_state(TASK_RUNNING);
	if (current->probed && queued(current) && current->waiting == wq_entry)
		sim_panic("a lost wake: %s's timed wait ended by its time, its condition true, with no wake",
			  current->comm);
	current->probed = false;
	if (!list_empty_careful(&wq_entry->entry)) {
		spin_lock(&wq_head->lock);
		list_del_init(&wq_entry->entry);
		spin_unlock(&wq_head->lock);
	}
	if (current->waiting == wq_entry)
		current->waiting = NULL;
}

/* The table of queues waits for a variable hash into, as kernel/sched/wait_bit.c's. */
#define SIM_VAR_WAIT_BITS 8
static struct wait_queue_head var_waitqueues[1 << SIM_VAR_WAIT_BITS];

static int __init init_var_waitqueues(void)
{
	int i;

	for (i = 0; i < ARRAY_SIZE(var_waitqueues); i++)
		init_waitqueue_head(&var_waitqueues[i]);
	return 0;
}
fs_initcall(init_var_waitqueues);

struct wait_queue_head *__var_waitqueue(void *p)
{
	return &var_waitqueues[hash_ptr(p, SIM_VAR_WAIT_BITS)];
}

static int var_wake_function(struct wait_queue_entry *wq_entry, unsigned int mode, int sync,
			     void *arg)
{
	struct wait_bit_key *key = arg;
	struct wait_bit_queue_entry *wbq_entry =
		container_of(wq_entry, struct wait_bit_queue_entry, wq_entry);

	if (wbq_entry->key.flags != key->flags || wbq_entry->key.bit_nr != key->bit_nr)
		return 0;
	return autoremove_wake_function(wq_entry, mode, sync, key);
}

void init_wait_var_entry(struct wait_bit_queue_entry *wbq_entry, void *var, int flags)
{
	*wbq_entry = (struct wait_bit_queue_entry) {
		.key = { .flags = var, .bit_nr = -1 },
		.wq_entry = {
			.flags = flags,
			.private = current,
			.func = var_wake_function,
			.entry = LIST_HEAD_INIT(wbq_entry->wq_entry.entry),
		},
	};
}

void wake_up_var(void *var)
{
	struct wait_bit_key key = { .flags = var, .bit_nr = -1 };

	__wake_up(__var_waitqueue(var), TASK_NORMAL, 1, &key);
}
