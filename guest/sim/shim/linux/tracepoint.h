/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* Tracepoints: each an empty function, so that the kernel's own trace headers compile. */
#ifndef _SIM_TRACEPOINT_H
#define _SIM_TRACEPOINT_H

#define TP_PROTO(...) __VA_ARGS__
#define TP_ARGS(...) __VA_ARGS__
#define TRACE_DEFINE_ENUM(x)
#define DECLARE_EVENT_CLASS(...)
#define DEFINE_EVENT(template, name, proto, args) static inline void trace_##name(proto) {}
#define TRACE_EVENT(name, proto, ...) static inline void trace_##name(proto) {}

#endif
