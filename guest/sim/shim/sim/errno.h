/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The kernel's errnos dax.c and fs/dax.c name, with Linux's values. */
#ifndef _SIM_ERRNO_H
#define _SIM_ERRNO_H

#define EPERM 1
#define ENOENT 2
#define EINTR 4
#define EIO 5
#define E2BIG 7
#define EAGAIN 11
#define ENOMEM 12
#define EFAULT 14
#define EBUSY 16
#define EEXIST 17
#define EXDEV 18
#define ENODEV 19
#define EINVAL 22
#define EFBIG 27
#define ENOSPC 28
#define EROFS 30
#define ERANGE 34
#define EDEADLK 35
#define ENOSYS 38
#define EOPNOTSUPP 95
#define ETIMEDOUT 110
#define ESTALE 116
#define EDQUOT 122
#define EHWPOISON 133
#define ERESTARTSYS 512

#endif
