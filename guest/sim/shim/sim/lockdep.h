/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * Lockdep's assertions: each task keeps what it holds (kernel/locks.c), so that an assertion fails
 * the run. Not its graph of lock orders: an inversion shows up here only as a deadlock, when a
 * seed plays it out, which the scheduler reports.
 */
#ifndef _SIM_LOCKDEP_H
#define _SIM_LOCKDEP_H

#include <sim/base.h>

struct lock_class_key {
	int unused;
};

/* A lock that exists only for lockdep. */
struct lockdep_map {
	const char *name;
};

bool sim_holds(const void *lock);
bool sim_holds_write(const void *lock);
void sim_held_add(const void *lock, bool write, const char *name);
void sim_held_del(const void *lock);

#define lockdep_init_map(map, n, key, sub) ((map)->name = (n))
#define lock_is_held(map) sim_holds(map)
#define rwsem_acquire(map, sub, try, ip) sim_held_add(map, true, (map)->name)
#define rwsem_acquire_read(map, sub, try, ip) sim_held_add(map, false, (map)->name)
#define rwsem_release(map, ip) sim_held_del(map)
#define lockdep_assert(cond) WARN(!(cond), "lockdep_assert(%s)", #cond)
#define lockdep_assert_held(lock) lockdep_assert(sim_holds(lock))
#define lockdep_assert_held_write(lock) lockdep_assert(sim_holds_write(lock))
#define lockdep_assert_held_read(lock) lockdep_assert(sim_holds(lock))
#define lockdep_assert_not_held(lock) lockdep_assert(!sim_holds(lock))
#define lockdep_is_held(lock) sim_holds(lock)
#define lockdep_set_class(lock, key) do { } while (0)
#define lockdep_set_class_and_name(lock, key, name) do { } while (0)
#define might_lock(lock) do { } while (0)

#endif
