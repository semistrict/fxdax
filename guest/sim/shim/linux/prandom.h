/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* A seeded generator, for fxfs.dax_delay_us: xorshift, not the kernel's, but as deterministic. */
#ifndef _SIM_PRANDOM_H
#define _SIM_PRANDOM_H

#include <sim/base.h>

struct rnd_state {
	u64 state;
};

static inline void prandom_seed_state(struct rnd_state *state, u64 seed)
{
	state->state = seed * 2 + 1;
}
static inline u32 prandom_u32_state(struct rnd_state *state)
{
	u64 x = state->state;

	x ^= x << 13;
	x ^= x >> 7;
	x ^= x << 17;
	state->state = x;
	return x >> 32;
}

#endif
