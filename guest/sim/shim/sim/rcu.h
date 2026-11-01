/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * RCU: readers count; callbacks run, in the scheduler, once no task is a reader, which is later
 * than a grace period need be but never sooner. A use after call_rcu's free, past readers, is
 * ASan's to find.
 */
#ifndef _SIM_RCU_H
#define _SIM_RCU_H

#include <sim/base.h>

typedef void (*rcu_callback_t)(struct rcu_head *head);
void rcu_read_lock(void);
void rcu_read_unlock(void);
void call_rcu(struct rcu_head *head, rcu_callback_t func);
void synchronize_rcu(void);

#define rcu_assign_pointer(p, v) WRITE_ONCE(p, (typeof(p))(v))
#define RCU_INIT_POINTER(p, v) rcu_assign_pointer(p, v)
#define rcu_dereference(p) READ_ONCE(p)
#define rcu_dereference_raw(p) READ_ONCE(p)
#define rcu_dereference_check(p, c) READ_ONCE(p)
#define rcu_dereference_protected(p, c) READ_ONCE(p)
#define rcu_access_pointer(p) READ_ONCE(p)
#define rcu_read_lock_held() 1
#define rcu_read_lock_sched() rcu_read_lock()
#define rcu_read_unlock_sched() rcu_read_unlock()

#endif
