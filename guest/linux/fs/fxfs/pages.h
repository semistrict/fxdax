/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * fxdax: the state of a range's pages, copying on write (doc-13, TASK-160.4): which of them have
 * a window page, which of those are zeros, which the host holds private, the VM's own, and which
 * are in their file's dirty set. Each page is one of
 *
 *	unmapped -> mapped (an extent's page, or zeros) -> private -> dirty
 *
 * a page dirty only if private, private only if mapped, and changes only by the transitions
 * below, each of what the window or the file does to it, and each checking that those hold: no
 * other code sees the state but through them and the queries after them (pages.c). What a
 * transition changes it says, for dax.c to count: the tallies of private and dirty pages, the
 * budget under private_max, the room reserved in Fxfs for dirty pages, and the requests to the
 * device the transitions follow, are dax.c's.
 *
 * Locking is dax.c's: the state is its range's file's, under the file's DAX semaphore, held for
 * writing for every transition but fxfs_dax_pages_dirty, which iomaps make holding it shared, one
 * beside another. Queries see what transitions under the semaphore as held by their caller allow:
 * holding it shared, private and dirty pages that iomaps beside them make so as they look, which
 * they read as the race it is, each word once (pages.c), a page they find so staying so.
 */
#ifndef _FXFS_PAGES_H
#define _FXFS_PAGES_H

#include <linux/types.h>

struct fxfs_dax_pages;

/* What a transition changed: how many pages it made private and dirty, or, below zero, took. */
struct fxfs_dax_change {
	long private, dirty;
};

/* The state of `nr` pages, none mapped, or NULL without the memory. */
struct fxfs_dax_pages *fxfs_dax_pages_new(unsigned long nr);
void fxfs_dax_pages_free(struct fxfs_dax_pages *pages);

/*
 * Transitions of the `n` pages from `first`, each as a request ended, or as an iomap is about to
 * store to them.
 */

/*
 * A MAP, a range's own or a slice's region's, put an extent's pages, or `zeros`, under them, none
 * private: mapped none, or mapped already, a map tried again.
 */
void fxfs_dax_pages_fill(struct fxfs_dax_pages *pages, unsigned long first, unsigned long n,
			 bool zeros);
/*
 * A rebind's MAP put their extent's pages, or `zeros`, under them, private and clean: the host
 * freed their private pages, the same bytes at the same address. How many were private.
 */
long fxfs_dax_pages_rebind(struct fxfs_dax_pages *pages, unsigned long first, unsigned long n,
			   bool zeros);
/*
 * An iomap about to store to them, their PRIVATE done: each private and dirty. What it made so of
 * them, others' iomaps beside it making some so meanwhile; fxfs_dax_pages_would_dirty says what it
 * would, before. With the file's semaphore held shared or more.
 */
struct fxfs_dax_change fxfs_dax_pages_dirty(struct fxfs_dax_pages *pages, unsigned long first,
					    unsigned long n);
/*
 * fxdax: a write's PRIVATE made them private ahead of its stores, clean (TASK-164.10): each
 * private. How many it made so, others' iomaps beside it making some so meanwhile;
 * fxfs_dax_pages_would_dirty's private says what it would, before. Each mapped, none unwritten.
 * With the file's semaphore held shared or more.
 */
long fxfs_dax_pages_private_ahead(struct fxfs_dax_pages *pages, unsigned long first,
				  unsigned long n);
/*
 * Writeback's swap: every dirty page is clean, private still, and in `set` (FXFS_DAX_PAGES bits,
 * as many as a range has pages at most), the writeback set: how many.
 */
unsigned long fxfs_dax_pages_swap(struct fxfs_dax_pages *pages, unsigned long *set);
/*
 * fxdax: a writeback that Fxfs had no memory for gives back `set`, of its writeback set: each page
 * of it private still dirty again, as before the swap, those a store dirtied again meanwhile
 * dirty already (TASK-207). How many it made dirty.
 */
unsigned long fxfs_dax_pages_redirty(struct fxfs_dax_pages *pages, const unsigned long *set);
/*
 * A truncate's MAP of zeros over them, past the file's end: zeros, their private pages freed by
 * the host, their dirty pages dropped. What it took.
 */
struct fxfs_dax_change fxfs_dax_pages_cut(struct fxfs_dax_pages *pages, unsigned long first,
					  unsigned long n);
/*
 * An UNMAP of them: nothing mapped, their private pages ended, their dirty ones dropped. What it
 * took.
 */
struct fxfs_dax_change fxfs_dax_pages_unmap(struct fxfs_dax_pages *pages, unsigned long first,
					    unsigned long n);

/* Queries. */

/* What dirtying the `n` pages from `first` would make private and dirty, of those not yet. */
struct fxfs_dax_change fxfs_dax_pages_would_dirty(const struct fxfs_dax_pages *pages,
						  unsigned long first, unsigned long n);
/* Whether any page is dirty: in its file's dirty set, which no extent holds yet. */
bool fxfs_dax_pages_any_dirty(const struct fxfs_dax_pages *pages);
/* Whether any page from `first` to `last`, exclusive, has a window page. */
bool fxfs_dax_pages_any_mapped(const struct fxfs_dax_pages *pages, unsigned long first,
			       unsigned long last);
/* Whether page `page` has a window page. */
bool fxfs_dax_pages_mapped(const struct fxfs_dax_pages *pages, unsigned long page);
/* The first page from `first` to `last`, exclusive, with no window page, or `last`. */
unsigned long fxfs_dax_pages_next_unmapped(const struct fxfs_dax_pages *pages,
					   unsigned long first, unsigned long last);
/* Where the run of pages from `first` mapped as it is, or unmapped as it is, ends, up to `last`. */
unsigned long fxfs_dax_pages_mapped_run(const struct fxfs_dax_pages *pages, unsigned long first,
					unsigned long last);
/* Whether page `page` maps zeros and was never written: a hole, or past the file's end. */
bool fxfs_dax_pages_unwritten(const struct fxfs_dax_pages *pages, unsigned long page);
/*
 * fxdax: where the run of pages from `first` that a PRIVATE may make private ahead of a write
 * ends, up to `last`: each mapped, and not unwritten, a hole's zeros (TASK-164.10).
 */
unsigned long fxfs_dax_pages_aheadable_run(const struct fxfs_dax_pages *pages, unsigned long first,
					   unsigned long last);
/* Where the run of pages from `first` unwritten as it is, or written, ends, up to `last`. */
unsigned long fxfs_dax_pages_unwritten_run(const struct fxfs_dax_pages *pages,
					   unsigned long first, unsigned long last);
/*
 * The first page from `from` that is private and clean, a rebind's to MAP, and in `*end` where
 * the run of such pages from it ends; or the number of pages, if there is none.
 */
unsigned long fxfs_dax_pages_next_rebindable(const struct fxfs_dax_pages *pages,
					     unsigned long from, unsigned long *end);

#endif
