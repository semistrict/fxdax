// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Locks, whose takings are the scheduling points. Each keeps its waiters in order and hands itself
 * over, as near to the kernel's rules as what they let tasks observe: a mutex is handed to a waiter
 * the seed picks (the kernel's order isn't fixed either), and a rwsem to its first waiter, with the
 * readers among those waiting, after which readers queue behind writers, so that a reader can wait
 * for a reader through a writer between them, which deadlocks recursive readers.
 */

#include <sim/sim.h>
#include <sim/host.h>

/* A task waiting for a lock, on its stack. */
struct waiter {
	struct list_head list;
	struct task_struct *task;
	bool write, granted;
};

static void block_on(struct waiter *waiter, const char *what, const void *lock)
{
	current->blocked_on = what;
	current->blocked_lock = lock;
	while (!READ_ONCE(waiter->granted)) {
		set_current_state(TASK_UNINTERRUPTIBLE);
		if (READ_ONCE(waiter->granted))
			break;
		schedule();
	}
	__set_current_state(TASK_RUNNING);
	current->blocked_on = NULL;
	current->blocked_lock = NULL;
}

void sim_held_add(const void *lock, bool write, const char *name)
{
	if (current->nr_held == SIM_MAX_HELD)
		sim_panic("%s holds too many locks", current->comm);
	current->held[current->nr_held].lock = lock;
	current->held[current->nr_held].name = name;
	current->held[current->nr_held++].write = write;
}

void sim_held_del(const void *lock)
{
	int i;

	for (i = current->nr_held - 1; i >= 0; i--) {
		if (current->held[i].lock != lock)
			continue;
		memmove(&current->held[i], &current->held[i + 1],
			(--current->nr_held - i) * sizeof(current->held[0]));
		return;
	}
	sim_panic("%s releases a lock it doesn't hold", current->comm);
}

bool sim_holds(const void *lock)
{
	int i;

	for (i = 0; i < current->nr_held; i++) {
		if (current->held[i].lock == lock)
			return true;
	}
	return false;
}

bool sim_holds_write(const void *lock)
{
	int i;

	for (i = 0; i < current->nr_held; i++) {
		if (current->held[i].lock == lock && current->held[i].write)
			return true;
	}
	return false;
}

void sim_spin_init(spinlock_t *lock, const char *name)
{
	lock->owner = NULL;
	lock->name = name;
}

bool spin_trylock(spinlock_t *lock)
{
	sim_point(lock->name);
	if (lock->owner)
		return false;
	lock->owner = current;
	current->atomic++;
	sim_held_add(lock, true, lock->name);
	return true;
}

/*
 * A holder may be switched out, as another CPU's would run on while this one spins: so a spin is
 * a task that doesn't run until the lock is free.
 */
void spin_lock(spinlock_t *lock)
{
	if (lock->owner == current)
		sim_panic("%s takes spinlock %s again", current->comm, lock->name);
	sim_point(lock->name);
	while (lock->owner) {
		current->blocked_on = "a spinlock";
		current->blocked_lock = lock;
		sim_point(lock->name);
	}
	current->blocked_on = NULL;
	current->blocked_lock = NULL;
	lock->owner = current;
	current->atomic++;
	sim_held_add(lock, true, lock->name);
}

void spin_unlock(spinlock_t *lock)
{
	if (lock->owner != current)
		sim_panic("%s unlocks spinlock %s it doesn't hold", current->comm, lock->name);
	lock->owner = NULL;
	current->atomic--;
	sim_held_del(lock);
}

void sim_mutex_init(struct mutex *lock, const char *name)
{
	lock->owner = NULL;
	lock->name = name;
	INIT_LIST_HEAD(&lock->waiters);
}

bool mutex_trylock(struct mutex *lock)
{
	sim_point(lock->name);
	if (lock->owner)
		return false;
	lock->owner = current;
	sim_held_add(lock, true, lock->name);
	return true;
}

void mutex_lock(struct mutex *lock)
{
	might_sleep();
	if (lock->owner == current)
		sim_panic("%s takes mutex %s again", current->comm, lock->name);
	sim_point(lock->name);
	if (lock->owner) {
		struct waiter waiter = { .task = current };

		list_add_tail(&waiter.list, &lock->waiters);
		block_on(&waiter, "a mutex", lock);
	}
	lock->owner = current;
	sim_held_add(lock, true, lock->name);
}

void mutex_unlock(struct mutex *lock)
{
	struct waiter *waiter, *pick = NULL;
	unsigned int count = 0, at;

	if (lock->owner != current)
		sim_panic("%s unlocks mutex %s it doesn't hold", current->comm, lock->name);
	sim_held_del(lock);
	lock->owner = NULL;
	list_for_each_entry(waiter, &lock->waiters, list)
		count++;
	if (!count)
		return;
	at = sim_random() % count;
	list_for_each_entry(waiter, &lock->waiters, list) {
		if (!at--) {
			pick = waiter;
			break;
		}
	}
	list_del(&pick->list);
	/* Handed over: taken already, as far as anyone else can see. */
	lock->owner = pick->task;
	pick->granted = true;
	wake_up_process(pick->task);
}

void sim_init_rwsem(struct rw_semaphore *sem, const char *name)
{
	sem->writer = NULL;
	sem->readers = 0;
	sem->name = name;
	INIT_LIST_HEAD(&sem->waiters);
}

/* Hands the free `sem` to its first waiter, and if that's a reader, to all the readers waiting. */
static void rwsem_grant(struct rw_semaphore *sem)
{
	struct waiter *waiter, *next;
	bool reader;

	if (sem->writer || sem->readers || list_empty(&sem->waiters))
		return;
	waiter = list_first_entry(&sem->waiters, struct waiter, list);
	reader = !waiter->write;
	list_for_each_entry_safe(waiter, next, &sem->waiters, list) {
		if (reader && waiter->write)
			continue;
		list_del(&waiter->list);
		if (reader) {
			sem->readers++;
		} else {
			sem->writer = waiter->task;
		}
		waiter->granted = true;
		wake_up_process(waiter->task);
		if (!reader)
			break;
	}
}

int down_read_trylock(struct rw_semaphore *sem)
{
	sim_point(sem->name);
	if (sem->writer || !list_empty(&sem->waiters))
		return 0;
	sem->readers++;
	sim_held_add(sem, false, sem->name);
	return 1;
}

int down_write_trylock(struct rw_semaphore *sem)
{
	sim_point(sem->name);
	if (sem->writer || sem->readers)
		return 0;
	sem->writer = current;
	sim_held_add(sem, true, sem->name);
	return 1;
}

void down_read(struct rw_semaphore *sem)
{
	might_sleep();
	sim_point(sem->name);
	if (sem->writer || !list_empty(&sem->waiters)) {
		struct waiter waiter = { .task = current };

		if (sem->writer == current)
			sim_panic("%s takes rwsem %s, which it holds for writing", current->comm,
				  sem->name);
		list_add_tail(&waiter.list, &sem->waiters);
		block_on(&waiter, "a rwsem, to read", sem);
	} else {
		sem->readers++;
	}
	sim_held_add(sem, false, sem->name);
}

void down_write(struct rw_semaphore *sem)
{
	might_sleep();
	sim_point(sem->name);
	if (sem->writer || sem->readers) {
		struct waiter waiter = { .task = current, .write = true };

		if (sem->writer == current)
			sim_panic("%s takes rwsem %s again", current->comm, sem->name);
		list_add_tail(&waiter.list, &sem->waiters);
		block_on(&waiter, "a rwsem, to write", sem);
	} else {
		sem->writer = current;
	}
	sim_held_add(sem, true, sem->name);
}

void up_read(struct rw_semaphore *sem)
{
	sim_held_del(sem);
	if (!--sem->readers)
		rwsem_grant(sem);
}

void up_write(struct rw_semaphore *sem)
{
	if (sem->writer != current)
		sim_panic("%s unlocks rwsem %s it doesn't hold", current->comm, sem->name);
	sim_held_del(sem);
	sem->writer = NULL;
	rwsem_grant(sem);
}
