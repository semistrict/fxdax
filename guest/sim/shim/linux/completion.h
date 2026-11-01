/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* Completions, on a wait queue rather than the kernel's simple one. */
#ifndef _SIM_COMPLETION_H
#define _SIM_COMPLETION_H

#include <linux/wait.h>

struct completion {
	unsigned int done;
	wait_queue_head_t wait;
};

static inline void init_completion(struct completion *x)
{
	x->done = 0;
	init_waitqueue_head(&x->wait);
}
static inline void complete(struct completion *x)
{
	x->done++;
	wake_up_all(&x->wait);
}
static inline void wait_for_completion(struct completion *x)
{
	wait_event(x->wait, x->done);
	x->done--;
}

#endif
