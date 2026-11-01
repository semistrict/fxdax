/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * Tasks and what they wait on. Simulated tasks are coroutines, of which one runs at a time; what
 * follows from that (locks taken and sleeps) are the scheduling points, where a seed picks the next
 * (kernel/sched.c). Time is virtual: a sleep costs nothing, and nothing depends on the host's clock.
 */
#ifndef _SIM_SCHED_H
#define _SIM_SCHED_H

#include <sim/atomic.h>
#include <sim/lockdep.h>
#include <sim/rcu.h>

#define TASK_RUNNING 0x0000
#define TASK_INTERRUPTIBLE 0x0001
#define TASK_UNINTERRUPTIBLE 0x0002
#define TASK_WAKEKILL 0x0100
#define TASK_NOLOAD 0x0400
#define TASK_NORMAL (TASK_INTERRUPTIBLE | TASK_UNINTERRUPTIBLE)
#define TASK_KILLABLE (TASK_WAKEKILL | TASK_UNINTERRUPTIBLE)
#define TASK_IDLE (TASK_UNINTERRUPTIBLE | TASK_NOLOAD)

#define MAX_SCHEDULE_TIMEOUT LONG_MAX
#define HZ 1000
#define NSEC_PER_SEC 1000000000L
#define NSEC_PER_MSEC 1000000L
#define NSEC_PER_USEC 1000L

#define SIM_MAX_HELD 32

struct sim_fiber;

/* A simulated task. */
struct task_struct {
	int pid;
	const char *comm;
	unsigned int __state;
	/* Whether it wants to die: a fatal signal the tests send. */
	bool killed;
	/* How often the fault it is in had the kernel drop entries: -1 out of faults. */
	int fault_drops;
	/* Kernel copies don't fault (fxfs_dax_write_iter's, and read's). */
	int pagefault_disabled;
	/* Spinlocks held, and RCU readers entered: a sleep under either fails the run. */
	int atomic;
	int rcu_depth;
	unsigned int nofs;
	/* What it holds, for lockdep's assertions and the reports of deadlocks. */
	struct {
		const void *lock;
		const char *name;
		bool write;
	} held[SIM_MAX_HELD];
	int nr_held;
	/* What it waits for: for the report of a deadlock. */
	const char *blocked_on;
	const void *blocked_lock;
	/* The virtual time a timed sleep ends, or 0. */
	u64 wake_ns;
	struct mm_struct *mm;
	struct sim_fiber *fiber;
	void (*fn)(void *arg);
	void *arg;
	bool done;
	struct list_head tasks;
};

extern struct task_struct *sim_current;
#define current sim_current
#define get_current() sim_current

#define __set_current_state(state) WRITE_ONCE(current->__state, (state))
#define set_current_state(state) WRITE_ONCE(current->__state, (state))
#define __set_task_state(task, state) WRITE_ONCE((task)->__state, (state))

void schedule(void);
long schedule_timeout(long timeout);
#define io_schedule() schedule()
#define io_schedule_timeout(t) schedule_timeout(t)
void cond_resched(void);
#define need_resched() false
int wake_up_process(struct task_struct *task);
void sim_might_sleep(const char *file, int line);
#define might_sleep() sim_might_sleep(__FILE__, __LINE__)
#define might_sleep_if(cond) do { if (cond) might_sleep(); } while (0)

static inline bool fatal_signal_pending(struct task_struct *task) { return task->killed; }
static inline bool signal_pending(struct task_struct *task) { return task->killed; }
static inline int signal_pending_state(unsigned int state, struct task_struct *task)
{
	return (state & (TASK_INTERRUPTIBLE | TASK_WAKEKILL)) && task->killed;
}

static inline void pagefault_disable(void) { current->pagefault_disabled++; }
static inline void pagefault_enable(void) { current->pagefault_disabled--; }
static inline bool pagefault_disabled(void) { return current->pagefault_disabled; }

static inline unsigned int memalloc_nofs_save(void)
{
	unsigned int was = current->nofs;

	current->nofs = 1;
	return was;
}
static inline void memalloc_nofs_restore(unsigned int was) { current->nofs = was; }

/* A spinlock is a lock like another here: a task that holds one may be switched out. */
typedef struct spinlock {
	struct task_struct *owner;
	const char *name;
} spinlock_t;

#define __SPIN_LOCK_UNLOCKED(n) { .name = #n }
#define DEFINE_SPINLOCK(x) spinlock_t x = __SPIN_LOCK_UNLOCKED(x)

void sim_spin_init(spinlock_t *lock, const char *name);
void spin_lock(spinlock_t *lock);
bool spin_trylock(spinlock_t *lock);
void spin_unlock(spinlock_t *lock);
#define spin_lock_init(lock) sim_spin_init(lock, #lock)
#define spin_lock_nested(lock, sub) spin_lock(lock)
#define spin_lock_irq(lock) spin_lock(lock)
#define spin_unlock_irq(lock) spin_unlock(lock)
#define spin_lock_bh(lock) spin_lock(lock)
#define spin_unlock_bh(lock) spin_unlock(lock)
#define spin_lock_irqsave(lock, flags) do { (flags) = 0; spin_lock(lock); } while (0)
#define spin_lock_irqsave_nested(lock, flags, sub) spin_lock_irqsave(lock, flags)
#define spin_unlock_irqrestore(lock, flags) do { (void)(flags); spin_unlock(lock); } while (0)
#define spin_trylock_irq(lock) spin_trylock(lock)
#define spin_is_locked(lock) (!!(lock)->owner)
#define assert_spin_locked(lock) WARN_ON(!sim_holds(lock))

struct mutex {
	struct task_struct *owner;
	struct list_head waiters;
	const char *name;
};

#define __MUTEX_INITIALIZER(lock) { .waiters = LIST_HEAD_INIT((lock).waiters), .name = #lock }
#define DEFINE_MUTEX(lock) struct mutex lock = __MUTEX_INITIALIZER(lock)

void sim_mutex_init(struct mutex *lock, const char *name);
void mutex_lock(struct mutex *lock);
bool mutex_trylock(struct mutex *lock);
void mutex_unlock(struct mutex *lock);
#define mutex_init(lock) sim_mutex_init(lock, #lock)
#define mutex_is_locked(lock) (!!(lock)->owner)

/* As the kernel's: new readers queue behind waiters, so that a reader may block on a reader. */
struct rw_semaphore {
	struct task_struct *writer;
	int readers;
	struct list_head waiters;
	const char *name;
};

void sim_init_rwsem(struct rw_semaphore *sem, const char *name);
void down_read(struct rw_semaphore *sem);
void down_write(struct rw_semaphore *sem);
int down_read_trylock(struct rw_semaphore *sem);
int down_write_trylock(struct rw_semaphore *sem);
void up_read(struct rw_semaphore *sem);
void up_write(struct rw_semaphore *sem);
#define init_rwsem(sem) sim_init_rwsem(sem, #sem)

#endif
