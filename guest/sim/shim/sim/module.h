/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * Module parameters: a table of them, by name, which tests set as a kernel's command line does
 * (sim_param_set): so that dax.c's fault injection, pause points and diagnostics work unchanged.
 */
#ifndef _SIM_MODULE_H
#define _SIM_MODULE_H

#include <sim/base.h>

struct kernel_param;
struct kernel_param_ops {
	int (*set)(const char *val, const struct kernel_param *kp);
	int (*get)(char *buffer, const struct kernel_param *kp);
};
struct kernel_param {
	const char *name;
	const struct kernel_param_ops *ops;
	void *arg;
};

int param_set_bool(const char *val, const struct kernel_param *kp);
int param_get_bool(char *buffer, const struct kernel_param *kp);
int param_set_uint(const char *val, const struct kernel_param *kp);
int param_get_uint(char *buffer, const struct kernel_param *kp);
int param_set_ulong(const char *val, const struct kernel_param *kp);
int param_get_ulong(char *buffer, const struct kernel_param *kp);
extern const struct kernel_param_ops param_ops_bool, param_ops_uint, param_ops_ulong;

#define module_param_cb(name_, ops_, arg_, perm)					\
	static const struct kernel_param __param_##name_ __used				\
	__section("sim_params") __aligned(8) = {					\
		.name = #name_, .ops = (ops_), .arg = (void *)(arg_) }
#define module_param_named(name_, value, type, perm) \
	module_param_cb(name_, &param_ops_##type, &(value), perm)
#define MODULE_PARM_DESC(name, description)

/* Sets the parameter `name` as `fxfs.name=value` would: 0, or the parameter's -errno. */
int sim_param_set(const char *name, const char *value);
/* A numeric parameter's value. */
unsigned long sim_param_read(const char *name);

#endif
