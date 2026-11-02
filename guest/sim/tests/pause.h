/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * dax.c's pause points (fxfs.dax_pause, TASK-119.9), as guest/tests' checks use them: a test plays
 * an interleaving out exactly. Each scenario here is a guest check's, so that the check's steps
 * are seen to reach what they mean to, where only a guest could tell otherwise.
 */
#ifndef _SIM_TESTS_PAUSE_H
#define _SIM_TESTS_PAUSE_H

#include <sim/sim.h>

/* enum fxfs_dax_step's. */
#define PAUSE_MOVED (1UL << 0)
#define PAUSE_RESEALED (1UL << 6)

#define check(cond, ...) do { if (!(cond)) sim_panic(__VA_ARGS__); } while (0)

static inline void pause_at(unsigned long steps)
{
	char value[24];

	snprintf(value, sizeof(value), "%lu", steps);
	sim_param_set("dax_pause", value);
}

/* Waits for a task to wait at `step`. */
static inline void pause_wait(unsigned long step)
{
	int waited;

	for (waited = 0; !(sim_param_read("dax_paused") & step); waited++) {
		check(waited < 10000, "no task waits at step %#lx", step);
		msleep(1);
	}
}

#endif
