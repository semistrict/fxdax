/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The kernel's integer types, for fs/fxfs's headers, over libc's. */
#ifndef _SHIM_LINUX_TYPES_H
#define _SHIM_LINUX_TYPES_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

/* As the kernel's, whose u64 is long long everywhere, for the model's printf formats. */
typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;
typedef long long s64;

#endif
