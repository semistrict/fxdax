// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * fxdax: the state of a range's pages (pages.h), as four bitmaps that nothing else sees: so that
 * whatever dax.c does to a page goes through a transition named for it, which keeps the rules of
 * doc-13 the bitmaps stand for. They were fields of a range that a dozen places read and wrote,
 * the rules holding by each one's care (TASK-160.4).
 */

#include <linux/bitmap.h>
#include <linux/bug.h>
#include <linux/slab.h>

#include "pages.h"

struct fxfs_dax_pages {
	unsigned long nr;
	/* Pages with a window page: an extent's, zeros, or a private page. */
	unsigned long *mapped;
	/* Of those, pages whose window page is zeros, never written: holes and past the end. */
	unsigned long *zeros;
	/* Of those, pages the host holds private, counted against private_max. */
	unsigned long *private;
	/* Of those, pages in their file's dirty set, which no extent holds yet. */
	unsigned long *dirty;
	unsigned long bits[];
};

struct fxfs_dax_pages *fxfs_dax_pages_new(unsigned long nr)
{
	size_t words = BITS_TO_LONGS(nr);
	struct fxfs_dax_pages *pages;

	pages = kzalloc(struct_size(pages, bits, 4 * words), GFP_NOFS);
	if (!pages)
		return NULL;
	pages->nr = nr;
	pages->mapped = pages->bits;
	pages->zeros = pages->mapped + words;
	pages->private = pages->zeros + words;
	pages->dirty = pages->private + words;
	return pages;
}

void fxfs_dax_pages_free(struct fxfs_dax_pages *pages)
{
	kfree(pages);
}

/*
 * WARNs if the rules don't hold of the pages from `first` to `last`, exclusive: dirty implies
 * private, and private and zeros imply mapped. With no transition beside: the file's semaphore
 * held for writing.
 */
static void fxfs_dax_pages_check(const struct fxfs_dax_pages *pages, unsigned long first,
				 unsigned long last)
{
	unsigned long word;

	for (word = BIT_WORD(first); word < BITS_TO_LONGS(last); word++) {
		unsigned long mapped = pages->mapped[word];

		WARN_ONCE(pages->dirty[word] & ~pages->private[word],
			  "fxfs: pages dirty but not private: %lx of word %lu", pages->dirty[word] &
			  ~pages->private[word], word);
		WARN_ONCE((pages->private[word] | pages->zeros[word]) & ~mapped,
			  "fxfs: pages private or zeros but not mapped: %lx of word %lu",
			  (pages->private[word] | pages->zeros[word]) & ~mapped, word);
	}
}

/* That the `n` pages from `first` are pages of the state: WARNs, and says false, if not. */
static bool fxfs_dax_pages_within(const struct fxfs_dax_pages *pages, unsigned long first,
				  unsigned long n)
{
	return !WARN_ONCE(first > pages->nr || n > pages->nr - first,
			  "fxfs: pages %lu+%lu of %lu", first, n, pages->nr);
}

/*
 * How many of the `n` bits from `first` of `map` are set, each word read once. Of `private` and
 * `dirty`, iomaps beside the caller may be setting bits (fxfs_dax_pages_dirty, the file's semaphore
 * held shared by both), atomically: a race, and a benign one, so read as one (READ_ONCE), not as
 * find_next_bit's plain loads (KCSAN, GCE r611 and r613). Only a transition under the semaphore
 * held for writing clears a bit, so what a caller holding it shared counts set stays set: a count
 * at least what was set as it began, at most what is set by its end. fxfs_dax_pages_would_dirty's
 * caller takes room for what it counts clear, and gives back what fxfs_dax_pages_dirty, whose
 * test_and_set_bit is the read that decides, then finds others made private or dirty meanwhile
 * (fxfs_dax_dirty).
 */
static unsigned long fxfs_dax_pages_weight(const unsigned long *map, unsigned long first,
					   unsigned long n)
{
	unsigned long word, last, count = 0;

	if (!n)
		return 0;
	last = BIT_WORD(first + n - 1);
	for (word = BIT_WORD(first); word <= last; word++) {
		unsigned long bits = READ_ONCE(map[word]);

		if (word == BIT_WORD(first))
			bits &= BITMAP_FIRST_WORD_MASK(first);
		if (word == last)
			bits &= BITMAP_LAST_WORD_MASK(first + n);
		count += hweight_long(bits);
	}
	return count;
}

void fxfs_dax_pages_fill(struct fxfs_dax_pages *pages, unsigned long first, unsigned long n,
			 bool zeros)
{
	if (!fxfs_dax_pages_within(pages, first, n))
		return;
	/*
	 * A private page's bytes are the host's private page alone: a MAP over it but a rebind's,
	 * which a writeback and a seal came before, loses them. One of a page mapped already is a
	 * map tried again, the first having met an extent open still (fxfs_dax_map_pages).
	 */
	WARN_ONCE(find_next_bit(pages->private, first + n, first) < first + n,
		  "fxfs: a MAP of pages %lu+%lu, private", first, n);
	bitmap_set(pages->mapped, first, n);
	if (zeros)
		bitmap_set(pages->zeros, first, n);
	else
		bitmap_clear(pages->zeros, first, n);
	fxfs_dax_pages_check(pages, first, first + n);
}

long fxfs_dax_pages_rebind(struct fxfs_dax_pages *pages, unsigned long first, unsigned long n,
			   bool zeros)
{
	unsigned long page;
	long rebound = 0;

	if (!fxfs_dax_pages_within(pages, first, n))
		return 0;
	/* A dirty page's stores are in no extent: a MAP over it loses them. */
	WARN_ONCE(find_next_bit(pages->dirty, first + n, first) < first + n,
		  "fxfs: a rebind's MAP of pages %lu+%lu, dirty", first, n);
	for (page = first; page < first + n; page++) {
		rebound += test_and_clear_bit(page, pages->private);
		if (zeros)
			set_bit(page, pages->zeros);
		else
			clear_bit(page, pages->zeros);
	}
	bitmap_set(pages->mapped, first, n);
	fxfs_dax_pages_check(pages, first, first + n);
	return rebound;
}

/*
 * Each page private first, then dirty, atomically, as iomaps beside each other dirty pages: so
 * that whoever finds a page dirty finds it private.
 */
struct fxfs_dax_change fxfs_dax_pages_dirty(struct fxfs_dax_pages *pages, unsigned long first,
					    unsigned long n)
{
	struct fxfs_dax_change made = {};
	unsigned long page;

	if (!fxfs_dax_pages_within(pages, first, n))
		return made;
	/* Every page of a range a write stores to has a window page (fxfs_dax_map_missing). */
	WARN_ONCE(fxfs_dax_pages_next_unmapped(pages, first, first + n) < first + n,
		  "fxfs: pages %lu+%lu dirtied, not mapped", first, n);
	for (page = first; page < first + n; page++) {
		made.private += !test_and_set_bit(page, pages->private);
		made.dirty += !test_and_set_bit(page, pages->dirty);
	}
	return made;
}

long fxfs_dax_pages_private_ahead(struct fxfs_dax_pages *pages, unsigned long first,
				  unsigned long n)
{
	unsigned long page;
	long made = 0;

	if (!fxfs_dax_pages_within(pages, first, n))
		return 0;
	WARN_ONCE(fxfs_dax_pages_aheadable_run(pages, first, first + n) < first + n,
		  "fxfs: pages %lu+%lu made private ahead, not all mapped and written", first, n);
	for (page = first; page < first + n; page++)
		made += !test_and_set_bit(page, pages->private);
	return made;
}

unsigned long fxfs_dax_pages_swap(struct fxfs_dax_pages *pages, unsigned long *set)
{
	unsigned long word, swapped = 0;

	fxfs_dax_pages_check(pages, 0, pages->nr);
	bitmap_zero(set, pages->nr);
	for (word = 0; word < BITS_TO_LONGS(pages->nr); word++) {
		set[word] = pages->dirty[word];
		pages->dirty[word] = 0;
		swapped += hweight_long(set[word]);
	}
	return swapped;
}

unsigned long fxfs_dax_pages_redirty(struct fxfs_dax_pages *pages, const unsigned long *set)
{
	unsigned long word, made = 0;

	fxfs_dax_pages_check(pages, 0, pages->nr);
	for (word = 0; word < BITS_TO_LONGS(pages->nr); word++) {
		unsigned long back = set[word] & pages->private[word] & ~pages->dirty[word];

		pages->dirty[word] |= back;
		made += hweight_long(back);
	}
	return made;
}

struct fxfs_dax_change fxfs_dax_pages_cut(struct fxfs_dax_pages *pages, unsigned long first,
					  unsigned long n)
{
	struct fxfs_dax_change took = {};

	if (!fxfs_dax_pages_within(pages, first, n))
		return took;
	took.private = -fxfs_dax_pages_weight(pages->private, first, n);
	took.dirty = -fxfs_dax_pages_weight(pages->dirty, first, n);
	bitmap_clear(pages->dirty, first, n);
	bitmap_clear(pages->private, first, n);
	bitmap_set(pages->zeros, first, n);
	bitmap_set(pages->mapped, first, n);
	fxfs_dax_pages_check(pages, first, first + n);
	return took;
}

struct fxfs_dax_change fxfs_dax_pages_unmap(struct fxfs_dax_pages *pages, unsigned long first,
					    unsigned long n)
{
	struct fxfs_dax_change took = {};

	if (!fxfs_dax_pages_within(pages, first, n))
		return took;
	took.private = -fxfs_dax_pages_weight(pages->private, first, n);
	took.dirty = -fxfs_dax_pages_weight(pages->dirty, first, n);
	bitmap_clear(pages->dirty, first, n);
	bitmap_clear(pages->private, first, n);
	bitmap_clear(pages->zeros, first, n);
	bitmap_clear(pages->mapped, first, n);
	fxfs_dax_pages_check(pages, first, first + n);
	return took;
}

struct fxfs_dax_change fxfs_dax_pages_would_dirty(const struct fxfs_dax_pages *pages,
						  unsigned long first, unsigned long n)
{
	struct fxfs_dax_change would = {};

	if (!fxfs_dax_pages_within(pages, first, n))
		return would;
	would.private = n - fxfs_dax_pages_weight(pages->private, first, n);
	would.dirty = n - fxfs_dax_pages_weight(pages->dirty, first, n);
	return would;
}

bool fxfs_dax_pages_any_dirty(const struct fxfs_dax_pages *pages)
{
	/* Writeback sizes its batch holding the semaphore shared, as iomaps dirty pages. */
	return fxfs_dax_pages_weight(pages->dirty, 0, pages->nr);
}

bool fxfs_dax_pages_any_mapped(const struct fxfs_dax_pages *pages, unsigned long first,
			       unsigned long last)
{
	return find_next_bit(pages->mapped, min(last, pages->nr), first) < min(last, pages->nr);
}

bool fxfs_dax_pages_mapped(const struct fxfs_dax_pages *pages, unsigned long page)
{
	return page < pages->nr && test_bit(page, pages->mapped);
}

unsigned long fxfs_dax_pages_next_unmapped(const struct fxfs_dax_pages *pages,
					   unsigned long first, unsigned long last)
{
	return find_next_zero_bit(pages->mapped, min(last, pages->nr), first);
}

unsigned long fxfs_dax_pages_mapped_run(const struct fxfs_dax_pages *pages, unsigned long first,
					unsigned long last)
{
	last = min(last, pages->nr);
	if (first >= last)
		return last;
	return test_bit(first, pages->mapped) ? find_next_zero_bit(pages->mapped, last, first) :
						find_next_bit(pages->mapped, last, first);
}

bool fxfs_dax_pages_unwritten(const struct fxfs_dax_pages *pages, unsigned long page)
{
	return page < pages->nr && test_bit(page, pages->zeros) &&
	       !test_bit(page, pages->private);
}

unsigned long fxfs_dax_pages_unwritten_run(const struct fxfs_dax_pages *pages,
					   unsigned long first, unsigned long last)
{
	bool unwritten = fxfs_dax_pages_unwritten(pages, first);
	unsigned long page;

	last = min(last, pages->nr);
	for (page = first + 1; page < last && fxfs_dax_pages_unwritten(pages, page) == unwritten;
	     page++)
		;
	return min(page, last);
}

unsigned long fxfs_dax_pages_aheadable_run(const struct fxfs_dax_pages *pages, unsigned long first,
					   unsigned long last)
{
	unsigned long page;

	last = min(last, pages->nr);
	for (page = first; page < last; page++) {
		if (!test_bit(page, pages->mapped) || fxfs_dax_pages_unwritten(pages, page))
			break;
	}
	return page;
}

unsigned long fxfs_dax_pages_next_rebindable(const struct fxfs_dax_pages *pages,
					     unsigned long from, unsigned long *end)
{
	unsigned long page = find_next_bit(pages->private, pages->nr, from);

	while (page < pages->nr && test_bit(page, pages->dirty))
		page = find_next_bit(pages->private, pages->nr, page + 1);
	*end = page;
	while (*end < pages->nr && test_bit(*end, pages->private) && !test_bit(*end, pages->dirty))
		(*end)++;
	return page;
}
