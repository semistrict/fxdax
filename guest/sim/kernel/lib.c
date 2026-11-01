// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/* The small libraries beneath everything: allocation, bitmaps, printing, parameters. */

#include <sim/sim.h>
#include <sim/host.h>

void *kmalloc(size_t size, gfp_t gfp)
{
	void *p;

	might_alloc(gfp);
	p = host_malloc(size);
	if (gfp & __GFP_ZERO)
		memset(p, 0, size);
	return p;
}

void *krealloc(const void *p, size_t size, gfp_t gfp)
{
	might_alloc(gfp);
	return host_realloc((void *)p, size);
}

void kfree(const void *p)
{
	host_free((void *)p);
}

struct kmem_cache {
	size_t size;
};

struct kmem_cache *kmem_cache_create_simple(const char *name, size_t size)
{
	struct kmem_cache *cache = kmalloc(sizeof(*cache), GFP_KERNEL);

	cache->size = size;
	return cache;
}

/* Zeroed, as the XArray's one cache's constructor has its nodes. */
void *kmem_cache_alloc(struct kmem_cache *cache, gfp_t gfp)
{
	return kzalloc(cache->size, gfp);
}

void kmem_cache_free(struct kmem_cache *cache, void *p)
{
	kfree(p);
}

/* lib/radix-tree.c's, which the XArray's nodes come from, as a task finishes with them. */
struct kmem_cache *radix_tree_node_cachep;

void radix_tree_node_rcu_free(struct rcu_head *head)
{
	struct xa_node *node = container_of(head, struct xa_node, rcu_head);

	kmem_cache_free(radix_tree_node_cachep, node);
}

static int __init init_radix_tree(void)
{
	radix_tree_node_cachep = kmem_cache_create_simple("radix_tree_node", sizeof(struct xa_node));
	return 0;
}
fs_initcall(init_radix_tree);

unsigned long find_next_bit(const unsigned long *addr, unsigned long size, unsigned long offset)
{
	while (offset < size) {
		unsigned long word = addr[BIT_WORD(offset)] & BITMAP_FIRST_WORD_MASK(offset);

		if (word)
			return min(size, (offset & ~(BITS_PER_LONG - 1)) + __ffs(word));
		offset = (offset | (BITS_PER_LONG - 1)) + 1;
	}
	return size;
}

unsigned long find_next_zero_bit(const unsigned long *addr, unsigned long size,
				 unsigned long offset)
{
	while (offset < size) {
		unsigned long word = ~addr[BIT_WORD(offset)] & BITMAP_FIRST_WORD_MASK(offset);

		if (word)
			return min(size, (offset & ~(BITS_PER_LONG - 1)) + __ffs(word));
		offset = (offset | (BITS_PER_LONG - 1)) + 1;
	}
	return size;
}

unsigned long *bitmap_zalloc(unsigned int nbits, gfp_t flags)
{
	return kzalloc(BITS_TO_LONGS(nbits) * sizeof(unsigned long), flags);
}

void bitmap_free(const unsigned long *bitmap)
{
	kfree(bitmap);
}

void bitmap_set(unsigned long *map, unsigned int start, unsigned int len)
{
	while (len--)
		__set_bit(start++, map);
}

void bitmap_clear(unsigned long *map, unsigned int start, unsigned int len)
{
	while (len--)
		__clear_bit(start++, map);
}

unsigned int bitmap_weight(const unsigned long *src, unsigned int nbits)
{
	unsigned int i, count = 0;

	for (i = 0; i < BITS_TO_LONGS(nbits); i++)
		count += hweight_long(src[i] & (i + 1 == BITS_TO_LONGS(nbits) ?
						BITMAP_LAST_WORD_MASK(nbits) : ~0UL));
	return count;
}

unsigned long bitmap_find_next_zero_area_off(unsigned long *map, unsigned long size,
					     unsigned long start, unsigned int nr,
					     unsigned long align_mask,
					     unsigned long align_offset)
{
	unsigned long index, end, i;

	for (;;) {
		index = find_next_zero_bit(map, size, start);
		index = ALIGN(index + align_offset, align_mask + 1) - align_offset;
		end = index + nr;
		if (end > size)
			return end;
		i = find_next_bit(map, end, index);
		if (i >= end)
			return index;
		start = i + 1;
	}
}

void sim_log(const char *format, ...)
{
	char line[1024];
	va_list args;
	int len;

	va_start(args, format);
	len = vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	host_write_log(line, min_t(size_t, len, sizeof(line) - 1));
}

void sim_panic(const char *format, ...)
{
	char line[1024];
	va_list args;

	va_start(args, format);
	vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	sim_log("sim: PANIC in %s: %s\n", current ? current->comm : "?", line);
	host_stack_trace();
	sim_log("sim: at %llu points, trace %llx\n", sim_points(), sim_trace_hash());
	host_exit(2);
}

void sim_warn(const char *file, int line, const char *format, ...)
{
	char text[1024];
	va_list args;

	va_start(args, format);
	vsnprintf(text, sizeof(text), format, args);
	va_end(args);
	sim_panic("WARN at %s:%d: %s", file, line, text);
}

void seq_printf(struct seq_file *m, const char *format, ...)
{
	char line[1024];
	va_list args;

	va_start(args, format);
	vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	sim_log("%s", line);
}

void seq_putc(struct seq_file *m, char c)
{
	sim_log("%c", c);
}

/* Parameters: the two kinds the source names, and a lookup by name. */
int param_set_bool(const char *val, const struct kernel_param *kp)
{
	*(bool *)kp->arg = val[0] == 'y' || val[0] == '1';
	return 0;
}

int param_get_bool(char *buffer, const struct kernel_param *kp)
{
	return snprintf(buffer, 4, "%c", *(bool *)kp->arg ? 'Y' : 'N');
}

int param_set_uint(const char *val, const struct kernel_param *kp)
{
	return sscanf(val, "%u", (unsigned int *)kp->arg) == 1 ? 0 : -EINVAL;
}

int param_get_uint(char *buffer, const struct kernel_param *kp)
{
	return snprintf(buffer, 24, "%u", *(unsigned int *)kp->arg);
}

int param_set_ulong(const char *val, const struct kernel_param *kp)
{
	return sscanf(val, "%lu", (unsigned long *)kp->arg) == 1 ? 0 : -EINVAL;
}

int param_get_ulong(char *buffer, const struct kernel_param *kp)
{
	return snprintf(buffer, 24, "%lu", *(unsigned long *)kp->arg);
}

const struct kernel_param_ops param_ops_bool = { param_set_bool, param_get_bool };
const struct kernel_param_ops param_ops_uint = { param_set_uint, param_get_uint };
const struct kernel_param_ops param_ops_ulong = { param_set_ulong, param_get_ulong };

int sim_param_set(const char *name, const char *value)
{
	extern const struct kernel_param __start_sim_params[], __stop_sim_params[];
	const struct kernel_param *param;

	for (param = __start_sim_params; param < __stop_sim_params; param++) {
		if (!strcmp(param->name, name))
			return param->ops->set(value, param);
	}
	sim_panic("no parameter %s", name);
}
