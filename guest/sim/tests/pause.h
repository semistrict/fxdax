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

/* What a test expects, void once a sweep's call has failed (kernel/sweep.c). */
#define check sim_expect

static inline void pause_at(unsigned long steps)
{
	char value[24];

	snprintf(value, sizeof(value), "%lu", steps);
	sim_param_set("dax_pause", value);
}

/* Waits for a task to wait at `step`. */
static inline void pause_wait(unsigned long step)
{
	sim_await(sim_param_read("dax_paused") & step, "no task waits at step %#lx", step);
}

#endif
