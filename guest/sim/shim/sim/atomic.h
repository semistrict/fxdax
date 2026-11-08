/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * Atomics, plain: one simulated task runs at a time, and none is switched out within one of these,
 * so each is atomic, in the sequentially consistent order the scheduler picks. Weaker orders the
 * hardware allows are not simulated.
 */
#ifndef _SIM_ATOMIC_H
#define _SIM_ATOMIC_H

#include <sim/base.h>

#define ATOMIC_INIT(i) { (i) }
#define ATOMIC64_INIT(i) { (i) }
#define ATOMIC_LONG_INIT(i) { (i) }

#define __SIM_ATOMIC(name, atomic, value)						\
static inline value name##_read(const atomic *v) { return v->counter; }			\
static inline void name##_set(atomic *v, value i) { v->counter = i; }			\
static inline void name##_add(value i, atomic *v) { v->counter += i; }			\
static inline void name##_sub(value i, atomic *v) { v->counter -= i; }			\
static inline void name##_inc(atomic *v) { v->counter++; }				\
static inline void name##_dec(atomic *v) { v->counter--; }				\
static inline value name##_add_return(value i, atomic *v) { return v->counter += i; }	\
static inline value name##_sub_return(value i, atomic *v) { return v->counter -= i; }	\
static inline value name##_inc_return(atomic *v) { return ++v->counter; }		\
static inline value name##_dec_return(atomic *v) { return --v->counter; }		\
static inline bool name##_dec_and_test(atomic *v) { return !--v->counter; }		\
static inline value name##_xchg(atomic *v, value new)					\
{											\
	value old = v->counter;								\
											\
	v->counter = new;								\
	return old;									\
}											\
static inline value name##_fetch_add(value i, atomic *v)				\
{											\
	value old = v->counter;								\
											\
	v->counter += i;								\
	return old;									\
}											\
static inline value name##_cmpxchg(atomic *v, value old, value new)			\
{											\
	value was = v->counter;								\
											\
	if (was == old)									\
		v->counter = new;							\
	return was;									\
}											\
static inline bool name##_try_cmpxchg(atomic *v, value *old, value new)		\
{											\
	value was = name##_cmpxchg(v, *old, new);					\
	bool done = was == *old;							\
											\
	*old = was;									\
	return done;									\
}

__SIM_ATOMIC(atomic, atomic_t, int)
__SIM_ATOMIC(atomic64, atomic64_t, s64)
__SIM_ATOMIC(atomic_long, atomic_long_t, long)

#define cmpxchg(ptr, old, new) ({				\
	typeof(*(ptr)) __old = (old), __was = *(ptr);		\
	if (__was == __old)					\
		*(ptr) = (new);					\
	__was; })
#define try_cmpxchg(ptr, oldp, new) ({				\
	typeof(oldp) __oldp = (oldp);				\
	typeof(*(ptr)) __was = *(ptr);				\
	bool __done = __was == *__oldp;				\
	if (__done)						\
		*(ptr) = (new);					\
	else							\
		*__oldp = __was;				\
	__done; })
#define xchg(ptr, new) ({ typeof(*(ptr)) __was = *(ptr); *(ptr) = (new); __was; })

static inline void refcount_set(refcount_t *r, int n) { atomic_set(&r->refs, n); }
static inline unsigned int refcount_read(const refcount_t *r) { return atomic_read(&r->refs); }
static inline void refcount_inc(refcount_t *r)
{
	WARN_ON(atomic_inc_return(&r->refs) <= 1);
}
static inline bool refcount_inc_not_zero(refcount_t *r)
{
	if (!atomic_read(&r->refs))
		return false;
	atomic_inc(&r->refs);
	return true;
}
static inline bool refcount_dec_and_test(refcount_t *r)
{
	WARN_ON(atomic_read(&r->refs) <= 0);
	return atomic_dec_and_test(&r->refs);
}

#endif
