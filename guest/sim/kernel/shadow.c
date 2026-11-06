// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The file model (TASK-149.18): what each file of a mount holds as the calls tasks made of it told
 * them, a byte at a time, as ztest's blocks know their tags. kernel/mount.c's system calls and
 * kernel/mm.c's user accesses tell it what they did, and it checks: a read(2) or a load reads what
 * the file was told it holds, and a successful fsync, and the end of a run's sync (sim_end), find
 * all of it in Fxfs's extents, and the size: every acknowledged write durable.
 *
 * A byte is known once a call that ended told what it holds: written, stored, zeroed by a truncate,
 * a punch or a growth, cloned from a known one. A call that fails, or whose outcome the bytes can't
 * say (a write from a task's memory, a read into it), leaves what it reached unknown, which nothing
 * checks until a call tells it again; as does one that overlapped another on a page in time: each
 * page counts the calls changing it under way, and when the last began or ended, by the file's
 * clock, so that a call knows whether it had the page to itself (`stable`). A read or an fsync
 * checks a page only if no call changed it while it went. The size is a cell of its own, alike.
 *
 * It takes no lock and is no scheduling point, so that a run is the same run with it: it only
 * looks. A file not made by sim_file_new starts with nothing known, its size neither.
 */

#include <sim/sim.h>
#include <sim/shadow.h>

struct shadow_page {
	u8 bytes[PAGE_SIZE];
	unsigned long known[PAGE_SIZE / BITS_PER_LONG];
	/* Calls under way changing it, and the file's clock as the last of them began or ended. */
	unsigned int busy;
	u64 changed;
};

struct sim_shadow {
	struct list_head list;
	u64 ino;
	struct shadow_page **pages;
	pgoff_t nr_pages;
	/* Its size, if known, a cell as a page is. */
	loff_t size;
	bool size_known;
	unsigned int size_busy;
	u64 size_changed;
	u64 clock;
	/* The clock as the last fsync that returned 0 began: what changed before is durable. */
	u64 durable;
	/* Whether a writeback of it failed, which left the window holding what Fxfs doesn't. */
	bool failed;
};

static struct sim_shadow *shadow_find(struct sim_mount *mount, u64 ino)
{
	struct sim_shadow *shadow;

	list_for_each_entry(shadow, &mount->shadows, list) {
		if (shadow->ino == ino)
			return shadow;
	}
	return NULL;
}

/* File `ino`'s, made with nothing known if it has none. */
static struct sim_shadow *shadow_get(struct sim_mount *mount, u64 ino)
{
	struct sim_shadow *shadow = shadow_find(mount, ino);

	if (shadow)
		return shadow;
	shadow = kzalloc(sizeof(*shadow), GFP_KERNEL);
	shadow->ino = ino;
	list_add_tail(&shadow->list, &mount->shadows);
	return shadow;
}

void sim_shadow_new(struct sim_mount *mount, u64 ino)
{
	shadow_get(mount, ino)->size_known = true;
}

void sim_shadow_free(struct sim_mount *mount)
{
	struct sim_shadow *shadow, *next;
	pgoff_t i;

	list_for_each_entry_safe(shadow, next, &mount->shadows, list) {
		for (i = 0; i < shadow->nr_pages; i++)
			kfree(shadow->pages[i]);
		kfree(shadow->pages);
		list_del(&shadow->list);
		kfree(shadow);
	}
}

static struct sim_shadow *shadow_of(struct file *file)
{
	struct inode *inode = file_inode(file);

	return shadow_get(container_of(inode->i_sb, struct sim_mount, sb), inode->i_ino);
}

/* Page `index`, made with nothing known if `make` and it has none; else NULL. */
static struct shadow_page *page_of(struct sim_shadow *shadow, pgoff_t index, bool make)
{
	if (index >= shadow->nr_pages) {
		pgoff_t more = max(index + 1, 2 * shadow->nr_pages);

		if (!make)
			return NULL;
		shadow->pages = krealloc(shadow->pages, more * sizeof(*shadow->pages), GFP_KERNEL);
		memset(shadow->pages + shadow->nr_pages, 0,
		       (more - shadow->nr_pages) * sizeof(*shadow->pages));
		shadow->nr_pages = more;
	}
	if (!shadow->pages[index] && make)
		shadow->pages[index] = kzalloc(sizeof(**shadow->pages), GFP_KERNEL);
	return shadow->pages[index];
}

static bool known(const struct shadow_page *page, size_t at)
{
	return page->known[at / BITS_PER_LONG] & (1UL << (at % BITS_PER_LONG));
}

/* Whether every byte of `page` is known. */
static bool all_known(const struct shadow_page *page)
{
	size_t i;

	for (i = 0; i < ARRAY_SIZE(page->known); i++) {
		if (~page->known[i])
			return false;
	}
	return true;
}

/* Has the `len` bytes at `at` of `page` known, or not: a word at a time. */
static void set_known(struct shadow_page *page, size_t at, size_t len, bool is)
{
	while (len) {
		size_t bit = at % BITS_PER_LONG, part = min_t(size_t, len, BITS_PER_LONG - bit);
		unsigned long mask = (part == BITS_PER_LONG ? ~0UL : (1UL << part) - 1) << bit;

		if (is)
			page->known[at / BITS_PER_LONG] |= mask;
		else
			page->known[at / BITS_PER_LONG] &= ~mask;
		at += part;
		len -= part;
	}
}

/*
 * A call that changes `start` to `end` of the file, or to as far as it has pages if `end` is -1,
 * and its size if `size`, begun: its pages, and the size, are busy with it until it ends.
 */
static void begin_change(struct sim_shadow_call *call, struct sim_shadow *shadow, loff_t start,
			 loff_t end, bool size)
{
	pgoff_t index;

	*call = (struct sim_shadow_call) {
		.shadow = shadow, .start = start, .size = size, .changes = true,
		.began = ++shadow->clock,
		.end = end >= 0 ? end : max_t(loff_t, start, (loff_t)shadow->nr_pages << PAGE_SHIFT),
	};
	for (index = start >> PAGE_SHIFT; index < DIV_ROUND_UP(call->end, PAGE_SIZE); index++) {
		struct shadow_page *page = page_of(shadow, index, true);

		page->busy++;
		page->changed = call->began;
	}
	if (size) {
		shadow->size_busy++;
		shadow->size_changed = call->began;
	}
}

/* A call that only looks at `start` to `end` of the file begun. */
static void begin_look(struct sim_shadow_call *call, struct sim_shadow *shadow, loff_t start,
		       loff_t end)
{
	*call = (struct sim_shadow_call) {
		.shadow = shadow, .start = start, .end = end, .began = shadow->clock,
	};
}

/* Whether nothing but `call` changed `page` while `call` went, so far. */
static bool stable(const struct sim_shadow_call *call, const struct shadow_page *page)
{
	if (call->changes)
		return page->busy == 1 && page->changed == call->began;
	return !page->busy && page->changed <= call->began;
}

static bool size_stable(const struct sim_shadow_call *call)
{
	const struct sim_shadow *shadow = call->shadow;

	if (call->changes && call->size)
		return shadow->size_busy == 1 && shadow->size_changed == call->began;
	return shadow->size_known && !shadow->size_busy && shadow->size_changed <= call->began;
}

/*
 * What `call` made of the `len` bytes at `at`: `bytes`' (zeros if NULL) if `is`, else unknown; each
 * page's known only if nothing else changed it meanwhile.
 */
static void tell(const struct sim_shadow_call *call, loff_t at, const void *bytes, size_t len,
		 bool is)
{
	size_t done = 0;

	while (done < len) {
		struct shadow_page *page = page_of(call->shadow, (at + done) >> PAGE_SHIFT, true);
		size_t in = offset_in_page(at + done);
		size_t part = min_t(size_t, PAGE_SIZE - in, len - done);
		bool now = is && stable(call, page);

		if (now && bytes)
			memcpy(page->bytes + in, (const u8 *)bytes + done, part);
		else if (now)
			memset(page->bytes + in, 0, part);
		set_known(page, in, part, now);
		done += part;
	}
}

static void forget(const struct sim_shadow_call *call, loff_t at, size_t len)
{
	tell(call, at, NULL, len, false);
}

/* The size `call` made, known if nothing else changed it meanwhile. */
static void tell_size(struct sim_shadow_call *call, loff_t size)
{
	struct sim_shadow *shadow = call->shadow;

	shadow->size_known = size >= 0 && size_stable(call);
	shadow->size = size;
}

/* A changing call's end: its pages, and the size, no longer busy with it. */
static void end_change(struct sim_shadow_call *call)
{
	struct sim_shadow *shadow = call->shadow;
	pgoff_t index;
	u64 now = ++shadow->clock;

	for (index = call->start >> PAGE_SHIFT; index < DIV_ROUND_UP(call->end, PAGE_SIZE); index++) {
		struct shadow_page *page = page_of(shadow, index, true);

		page->busy--;
		page->changed = now;
	}
	if (call->size) {
		shadow->size_busy--;
		shadow->size_changed = now;
	}
}

/* The size, as `call` may count on it: known, and no other call changing it; else -1. */
static loff_t size_now(const struct sim_shadow_call *call)
{
	const struct sim_shadow *shadow = call->shadow;

	if (!shadow->size_known || shadow->size_busy > (call->changes && call->size))
		return -1;
	return shadow->size;
}

/*
 * That the `len` bytes `got`, read at `at` by `call`, are what the file holds where it knows,
 * unless something changed them meanwhile: `what` read them.
 */
static void check(const struct sim_shadow_call *call, const char *what, loff_t at, const u8 *got,
		  size_t len)
{
	size_t done = 0;

	while (done < len) {
		const struct shadow_page *page = page_of(call->shadow, (at + done) >> PAGE_SHIFT,
							 false);
		size_t in = offset_in_page(at + done), i;
		size_t part = min_t(size_t, PAGE_SIZE - in, len - done);
		bool whole = page && all_known(page);

		if (whole && !memcmp(page->bytes + in, got + done, part))
			page = NULL;
		for (i = 0; page && stable(call, page) && i < part; i++) {
			if (known(page, in + i) && page->bytes[in + i] != got[done + i])
				sim_panic("the file model: %s of ino %llu: byte %lld is %#x, where the file was told it holds %#x",
					  what, call->shadow->ino, at + done + i, got[done + i],
					  page->bytes[in + i]);
		}
		done += part;
	}
}

void sim_shadow_write_begin(struct sim_shadow_call *call, struct file *file, loff_t pos,
			    size_t len)
{
	struct sim_shadow *shadow = shadow_of(file);
	loff_t size = shadow->size_known ? shadow->size : -1;

	/* From the old end, if it is before: a growth zeros what is between. */
	begin_change(call, shadow, size >= 0 ? min(pos, size) : pos, pos + len,
		     size < 0 || pos + (loff_t)len > size);
	call->pos = pos;
	call->len = len;
}

void sim_shadow_write_end(struct sim_shadow_call *call, const void *bytes, ssize_t done)
{
	loff_t old = size_now(call), pos = call->pos;
	size_t wrote = done > 0 ? done : 0;

	if (wrote && old >= 0 && pos > old)
		tell(call, old, NULL, pos - old, true);
	tell(call, pos, bytes, wrote, bytes);
	forget(call, pos + wrote, call->len - wrote);
	/* A write that wrote nothing leaves the size as it was. */
	if (call->size)
		tell_size(call, old < 0 ? -1 : wrote ? max_t(loff_t, old, pos + wrote) : old);
	end_change(call);
}

void sim_shadow_read_begin(struct sim_shadow_call *call, struct file *file, loff_t pos, size_t len)
{
	begin_look(call, shadow_of(file), pos, pos + len);
}

void sim_shadow_read_end(struct sim_shadow_call *call, const char *what, const void *bytes,
			 ssize_t done)
{
	if (done > 0)
		check(call, what, call->start, bytes, done);
}

void sim_shadow_resize_begin(struct sim_shadow_call *call, struct file *file, loff_t size)
{
	struct sim_shadow *shadow = shadow_of(file);
	loff_t old = shadow->size_known ? shadow->size : -1;

	/* From the page the lower end is in, which a cut zeros past the end, or a failure may. */
	begin_change(call, shadow, round_down(old < 0 ? size : min(old, size), PAGE_SIZE),
		     old < 0 ? -1 : max(old, size), true);
	call->pos = size;
}

void sim_shadow_resize_end(struct sim_shadow_call *call, int err)
{
	loff_t old = size_now(call), size = call->pos;

	if (err) {
		forget(call, call->start, call->end - call->start);
		tell_size(call, -1);
	} else {
		/* Past the end, nothing; what a growth reaches, zeros, if the old end is known. */
		if (call->end > size)
			forget(call, size, call->end - size);
		if (old >= 0 && size > old)
			tell(call, old, NULL, size - old, true);
		tell_size(call, size);
	}
	end_change(call);
}

void sim_shadow_punch_begin(struct sim_shadow_call *call, struct file *file, loff_t start,
			    loff_t end)
{
	begin_change(call, shadow_of(file), start, end, false);
}

/* Zeros, within the size: past it, nothing of the file's. */
void sim_shadow_punch_end(struct sim_shadow_call *call, int err)
{
	loff_t size = size_now(call);

	forget(call, call->start, call->end - call->start);
	if (!err && size > call->start)
		tell(call, call->start, NULL, min(call->end, size) - call->start, true);
	end_change(call);
}

void sim_shadow_clone_begin(struct sim_shadow_clone *clone, struct file *in, loff_t pos_in,
			    struct file *out, loff_t pos_out, loff_t len)
{
	struct sim_shadow *to = shadow_of(out);
	loff_t size = to->size_known ? to->size : -1;

	begin_look(&clone->from, shadow_of(in), pos_in, pos_in + len);
	begin_change(&clone->to, to, size >= 0 ? min(pos_out, size) : pos_out, pos_out + len,
		     size < 0 || pos_out + len > size);
	clone->to.pos = pos_out;
	clone->to.len = len;
}

void sim_shadow_clone_end(struct sim_shadow_clone *clone, int err)
{
	struct sim_shadow_call *from = &clone->from, *to = &clone->to;
	loff_t old = size_now(to), from_size = size_now(from), at;

	if (err) {
		forget(to, to->start, to->end - to->start);
		if (to->size)
			tell_size(to, -1);
		end_change(to);
		return;
	}
	if (old >= 0 && to->pos > old)
		tell(to, old, NULL, to->pos - old, true);
	/* Each byte the source knows, and holds, within its size: a block past it holds what Fxfs has. */
	for (at = 0; at < to->len; at++) {
		const struct shadow_page *page = page_of(from->shadow, (from->start + at) >> PAGE_SHIFT,
							 false);
		size_t in = offset_in_page(from->start + at);
		bool is = page && stable(from, page) && known(page, in) && from_size >= 0 &&
			  from->start + at < from_size;

		tell(to, to->pos + at, is ? &page->bytes[in] : NULL, 1, is);
	}
	if (to->size)
		tell_size(to, old < 0 ? -1 : max(old, to->pos + to->len));
	end_change(to);
}

/* A user access's piece within one mapping: `file`'s `len` bytes at `pos`. */
void sim_shadow_store_begin(struct sim_shadow_call *call, struct file *file, loff_t pos, size_t len)
{
	begin_change(call, shadow_of(file), pos, pos + len, false);
	call->pos = pos;
	call->len = len;
}

/*
 * The `done` bytes that landed of the store: known within the size, as a store past the end isn't
 * the file's; the rest as they were. `bytes` NULL: unknown, as a read's into a mapping.
 */
void sim_shadow_store_end(struct sim_shadow_call *call, const void *bytes, size_t done)
{
	loff_t size = size_now(call), within;

	if (size < 0) {
		forget(call, call->pos, done);
	} else {
		within = clamp_t(loff_t, size - call->pos, 0, done);
		tell(call, call->pos, bytes, within, bytes);
	}
	end_change(call);
}

void sim_shadow_sync_begin(struct sim_shadow_call *call, struct file *file)
{
	begin_look(call, shadow_of(file), 0, -1);
}

/*
 * That Fxfs's extents hold every byte the file knows, and its size, but where a call changed them
 * since `call` began: an fsync's, or the end of a run's sync's, that returned 0.
 */
static void check_durable(const struct sim_shadow_call *call, struct fxfs *fs, const char *what)
{
	struct sim_shadow *shadow = call->shadow;
	loff_t size = shadow->size_known && !shadow->size_busy &&
		      shadow->size_changed <= call->began ? shadow->size : -1;
	u8 held[PAGE_SIZE];
	pgoff_t index;
	size_t i;

	if (size >= 0 && fxfs_sim_size(fs, shadow->ino) != size)
		sim_panic("the file model: after %s, Fxfs has ino %llu %llu bytes long, where it was told %lld",
			  what, shadow->ino, fxfs_sim_size(fs, shadow->ino), size);
	for (index = 0; index < shadow->nr_pages; index++) {
		const struct shadow_page *page = shadow->pages[index];

		if (!page || !stable(call, page))
			continue;
		fxfs_sim_read(fs, shadow->ino, (loff_t)index << PAGE_SHIFT, held, PAGE_SIZE);
		if (all_known(page) && !memcmp(held, page->bytes, PAGE_SIZE))
			continue;
		for (i = 0; i < PAGE_SIZE; i++) {
			if (known(page, i) && held[i] != page->bytes[i])
				sim_panic("the file model: after %s, Fxfs's extents hold %#x at byte %lld of ino %llu, where the file was told %#x: an acknowledged write lost",
					  what, held[i], ((loff_t)index << PAGE_SHIFT) + i,
					  shadow->ino, page->bytes[i]);
		}
	}
}

/*
 * A writeback's error, as it fails, or told, by an fsync or a sync: what changed since the file was
 * last durable may be lost, as the page cache's pages are clean after a failed writeback, which the
 * next fsync doesn't write again, and which a rebind, a drop or a truncate may then replace with
 * what Fxfs holds. And what calls under way change, which a writeback swapped out before they end.
 */
static void lost(struct sim_shadow *shadow)
{
	u64 now = ++shadow->clock;
	pgoff_t index;

	for (index = 0; index < shadow->nr_pages; index++) {
		struct shadow_page *page = shadow->pages[index];

		if (!page || (!page->busy && page->changed <= shadow->durable))
			continue;
		memset(page->known, 0, sizeof(page->known));
		if (page->busy)
			page->changed = now;
	}
	if (shadow->size_busy || shadow->size_changed > shadow->durable)
		shadow->size_known = false;
	if (shadow->size_busy)
		shadow->size_changed = now;
}

void sim_shadow_wb_failed(struct address_space *mapping)
{
	struct inode *inode = mapping->host;
	struct sim_shadow *shadow = shadow_get(container_of(inode->i_sb, struct sim_mount, sb),
					       inode->i_ino);

	lost(shadow);
	shadow->failed = true;
}

bool sim_shadow_failed(struct sim_mount *mount, u64 ino)
{
	struct sim_shadow *shadow = shadow_find(mount, ino);

	return shadow && shadow->failed;
}

void sim_shadow_sync_end(struct sim_shadow_call *call, struct fxfs *fs, int err)
{
	if (err) {
		lost(call->shadow);
		return;
	}
	check_durable(call, fs, "an fsync");
	call->shadow->durable = max(call->shadow->durable, call->began);
}

void sim_shadow_lost(struct sim_mount *mount)
{
	struct sim_shadow *shadow;

	list_for_each_entry(shadow, &mount->shadows, list)
		lost(shadow);
}

void sim_shadow_check_all(struct sim_mount *mount, const char *what)
{
	struct sim_shadow *shadow;
	struct sim_shadow_call call;

	list_for_each_entry(shadow, &mount->shadows, list) {
		begin_look(&call, shadow, 0, -1);
		check_durable(&call, mount->info->fs, what);
	}
}

bool sim_shadow_size(struct sim_mount *mount, u64 ino, loff_t *size)
{
	struct sim_shadow *shadow = shadow_find(mount, ino);

	if (!shadow || !shadow->size_known || shadow->size_busy)
		return false;
	*size = shadow->size;
	return true;
}
