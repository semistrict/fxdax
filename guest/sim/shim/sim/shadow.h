/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The file model (kernel/shadow.c): what kernel/mount.c's system calls and kernel/mm.c's user
 * accesses tell it, each as a call begun and ended around what it does, and what it checks.
 */
#ifndef _SIM_SHADOW_H
#define _SIM_SHADOW_H

#include <sim/sim.h>

struct sim_shadow;

/* One call's, under way. */
struct sim_shadow_call {
	struct sim_shadow *shadow;
	/* What of the file it may change, or look at, and its own place and length. */
	loff_t start, end, pos;
	size_t len;
	/* The file's clock as it began; whether it may change the size, and whether it changes. */
	u64 began;
	bool size, changes;
};

/* A clone's: a look at its source, and a change of its destination. */
struct sim_shadow_clone {
	struct sim_shadow_call from, to;
};

/* A file made, empty; and the files' models, as the mount goes. */
void sim_shadow_new(struct sim_mount *mount, u64 ino);
void sim_shadow_free(struct sim_mount *mount);

/* A write(2) of `len` bytes at `pos`, which wrote `done`: from `bytes`, or NULL for a task's memory. */
void sim_shadow_write_begin(struct sim_shadow_call *call, struct file *file, loff_t pos, size_t len);
void sim_shadow_write_end(struct sim_shadow_call *call, const void *bytes, ssize_t done);
/* A read(2) into `bytes`, which read `done` of them: checked, as `what`. */
void sim_shadow_read_begin(struct sim_shadow_call *call, struct file *file, loff_t pos, size_t len);
void sim_shadow_read_end(struct sim_shadow_call *call, const char *what, const void *bytes,
			 ssize_t done);
void sim_shadow_resize_begin(struct sim_shadow_call *call, struct file *file, loff_t size);
void sim_shadow_resize_end(struct sim_shadow_call *call, int err);
void sim_shadow_punch_begin(struct sim_shadow_call *call, struct file *file, loff_t start,
			    loff_t end);
void sim_shadow_punch_end(struct sim_shadow_call *call, int err);
void sim_shadow_clone_begin(struct sim_shadow_clone *clone, struct file *in, loff_t pos_in,
			    struct file *out, loff_t pos_out, loff_t len);
void sim_shadow_clone_end(struct sim_shadow_clone *clone, int err);
/*
 * A user access's store, within one mapping, of which `done` bytes landed: `bytes`', or unknown if
 * NULL, as a read's into the mapping.
 */
void sim_shadow_store_begin(struct sim_shadow_call *call, struct file *file, loff_t pos, size_t len);
void sim_shadow_store_end(struct sim_shadow_call *call, const void *bytes, size_t done);
/* An fsync, which, if it returned 0, has Fxfs hold all the file knows of. */
void sim_shadow_sync_begin(struct sim_shadow_call *call, struct file *file);
void sim_shadow_sync_end(struct sim_shadow_call *call, struct fxfs *fs, int err);
/*
 * A user access of `len` bytes at `addr`, a store or a load, through mappings of files, as kernel/mm.c
 * tells the model of it, each mapping's piece a call: `done` bytes of it landed or read, from or into
 * `bytes`, NULL for what a read stores, unknown.
 */
#define SIM_ACCESS_PIECES 4
struct sim_user_access {
	struct {
		struct sim_shadow_call call;
		size_t at, len;
	} pieces[SIM_ACCESS_PIECES];
	int count;
	bool store;
};
void sim_user_access_begin(struct sim_user_access *access, unsigned long addr, size_t len,
			   bool store);
void sim_user_access_end(struct sim_user_access *access, const void *bytes, size_t done);

/* A sync's error: what changed since each file was last durable may be lost. */
void sim_shadow_lost(struct sim_mount *mount);
/* That Fxfs holds all every file knows of, after `what`, with no call under way. */
void sim_shadow_check_all(struct sim_mount *mount, const char *what);
/* Whether a writeback of file `ino` failed: its reads may differ from Fxfs's extents for good. */
bool sim_shadow_failed(struct sim_mount *mount, u64 ino);
/* File `ino`'s size, if known, and no call changing it. */
bool sim_shadow_size(struct sim_mount *mount, u64 ino, loff_t *size);

#endif
