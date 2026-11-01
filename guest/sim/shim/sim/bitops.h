/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* Bit operations, bitmaps and bit searches, on arrays of longs. See sim/atomic.h for atomicity. */
#ifndef _SIM_BITOPS_H
#define _SIM_BITOPS_H

#include <sim/atomic.h>

static inline void set_bit(unsigned long nr, volatile unsigned long *addr)
{
	addr[BIT_WORD(nr)] |= BIT_MASK(nr);
}
static inline void clear_bit(unsigned long nr, volatile unsigned long *addr)
{
	addr[BIT_WORD(nr)] &= ~BIT_MASK(nr);
}
static inline bool test_bit(unsigned long nr, const volatile unsigned long *addr)
{
	return addr[BIT_WORD(nr)] & BIT_MASK(nr);
}
static inline bool test_and_set_bit(unsigned long nr, volatile unsigned long *addr)
{
	bool was = test_bit(nr, addr);

	set_bit(nr, addr);
	return was;
}
static inline bool test_and_clear_bit(unsigned long nr, volatile unsigned long *addr)
{
	bool was = test_bit(nr, addr);

	clear_bit(nr, addr);
	return was;
}
#define test_bit_acquire test_bit
#define clear_bit_unlock clear_bit
#define __set_bit set_bit
#define __clear_bit clear_bit
#define __test_and_set_bit test_and_set_bit
#define __test_and_clear_bit test_and_clear_bit

static inline unsigned long __ffs(unsigned long word) { return __builtin_ctzl(word); }
static inline unsigned long ffz(unsigned long word) { return __builtin_ctzl(~word); }
static inline int fls(unsigned int x) { return x ? 32 - __builtin_clz(x) : 0; }
static inline int fls64(u64 x) { return x ? 64 - __builtin_clzll(x) : 0; }
static inline unsigned long __fls(unsigned long word) { return 63 - __builtin_clzl(word); }
static inline int ilog2(u64 n) { return fls64(n) - 1; }
static inline bool is_power_of_2(unsigned long n) { return n && !(n & (n - 1)); }
static inline unsigned long roundup_pow_of_two(unsigned long n) { return 1UL << fls64(n - 1); }
static inline unsigned long hweight_long(unsigned long w) { return __builtin_popcountl(w); }
static inline int order_base_2(unsigned long n) { return n > 1 ? fls64(n - 1) : 0; }

unsigned long find_next_bit(const unsigned long *addr, unsigned long size, unsigned long offset);
unsigned long find_next_zero_bit(const unsigned long *addr, unsigned long size,
				 unsigned long offset);
#define find_first_bit(addr, size) find_next_bit(addr, size, 0)
#define find_first_zero_bit(addr, size) find_next_zero_bit(addr, size, 0)
#define for_each_set_bit(bit, addr, size) \
	for ((bit) = 0; (bit) = find_next_bit((addr), (size), (bit)), (bit) < (size); (bit)++)

#define bitmap_zalloc(nbits, flags) \
	((unsigned long *)kzalloc(BITS_TO_LONGS(nbits) * sizeof(unsigned long), flags))
void bitmap_free(const unsigned long *bitmap);
void bitmap_set(unsigned long *map, unsigned int start, unsigned int len);
void bitmap_clear(unsigned long *map, unsigned int start, unsigned int len);
unsigned int bitmap_weight(const unsigned long *src, unsigned int nbits);
unsigned long bitmap_find_next_zero_area_off(unsigned long *map, unsigned long size,
					     unsigned long start, unsigned int nr,
					     unsigned long align_mask,
					     unsigned long align_offset);

#define bitmap_find_next_zero_area(map, size, start, nr, align_mask) \
	bitmap_find_next_zero_area_off(map, size, start, nr, align_mask, 0)
#define BITMAP_FIRST_WORD_MASK(start) (~0UL << ((start) & (BITS_PER_LONG - 1)))
#define BITMAP_LAST_WORD_MASK(nbits) (~0UL >> (-(nbits) & (BITS_PER_LONG - 1)))

static inline void bitmap_zero(unsigned long *dst, unsigned int nbits)
{
	memset(dst, 0, BITS_TO_LONGS(nbits) * sizeof(unsigned long));
}
static inline void bitmap_fill(unsigned long *dst, unsigned int nbits)
{
	memset(dst, 0xff, BITS_TO_LONGS(nbits) * sizeof(unsigned long));
}
static inline bool bitmap_empty(const unsigned long *src, unsigned int nbits)
{
	return find_first_bit(src, nbits) >= nbits;
}
static inline bool bitmap_full(const unsigned long *src, unsigned int nbits)
{
	return find_first_zero_bit(src, nbits) >= nbits;
}

#endif
