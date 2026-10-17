/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * fxdax: the thread-local values of fs/fxfs's Rust, which a task's `fxfs_tls` holds
 * (guest/linux/task-fxfs-tls.patch), and which are dropped as the task exits.
 */
#ifndef _LINUX_FXFS_TLS_H
#define _LINUX_FXFS_TLS_H

struct task_struct;

#ifdef CONFIG_FXFS_FS
void fxfs_rt_tls_exit(struct task_struct *tsk);
#else
static inline void fxfs_rt_tls_exit(struct task_struct *tsk)
{
}
#endif

#endif
