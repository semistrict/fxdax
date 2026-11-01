/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The virtual clock, which runs only as tasks reach scheduling points or all of them sleep. */
#ifndef _SIM_TIME_H
#define _SIM_TIME_H

#include <sim/sched.h>

u64 ktime_get_ns(void);
void msleep(unsigned int ms);
void fsleep(unsigned long us);
#define usleep_range(a, b) fsleep(a)
#define jiffies (ktime_get_ns() / (NSEC_PER_SEC / HZ))
static inline unsigned long msecs_to_jiffies(unsigned int ms) { return ms; }
static inline unsigned long usecs_to_jiffies(unsigned int us) { return DIV_ROUND_UP(us, 1000); }

#endif
