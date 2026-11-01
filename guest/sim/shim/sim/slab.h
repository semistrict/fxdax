/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/* The kernel's allocators, the host's malloc, which ASan watches. */
#ifndef _SIM_SLAB_H
#define _SIM_SLAB_H

#include <sim/sched.h>

#define ___GFP_KSWAPD_RECLAIM 0x01u
#define ___GFP_DIRECT_RECLAIM 0x02u
#define __GFP_FS 0x04u
#define __GFP_IO 0x08u
#define __GFP_ZERO 0x10u
#define __GFP_NOFAIL 0x20u
#define __GFP_HIGHMEM 0x40u
#define __GFP_NOWARN 0x80u
#define __GFP_ACCOUNT 0x100u
#define __GFP_DIRECT_RECLAIM ___GFP_DIRECT_RECLAIM
#define __GFP_RECLAIM (___GFP_KSWAPD_RECLAIM | ___GFP_DIRECT_RECLAIM)
#define __GFP_BITS_SHIFT 12
#define __GFP_BITS_MASK ((1u << __GFP_BITS_SHIFT) - 1)
#define GFP_ATOMIC 0u
#define GFP_NOWAIT ___GFP_KSWAPD_RECLAIM
#define GFP_NOIO __GFP_RECLAIM
#define GFP_NOFS (__GFP_RECLAIM | __GFP_IO)
#define GFP_KERNEL (__GFP_RECLAIM | __GFP_IO | __GFP_FS)
#define GFP_USER GFP_KERNEL
#define GFP_HIGHUSER_MOVABLE (GFP_KERNEL | __GFP_HIGHMEM)
#define GFP_ZONEMASK 0u
#define gfpflags_allow_blocking(gfp) (!!((gfp) & __GFP_DIRECT_RECLAIM))
#define might_alloc(gfp) might_sleep_if(gfpflags_allow_blocking(gfp))

void *kmalloc(size_t size, gfp_t gfp);
void *krealloc(const void *p, size_t size, gfp_t gfp);
void kfree(const void *p);
#define kmalloc_noprof kmalloc
#define kzalloc(size, gfp) kmalloc(size, (gfp) | __GFP_ZERO)
#define kmalloc_array(n, size, gfp) kmalloc((n) * (size), gfp)
#define kcalloc(n, size, gfp) kzalloc((n) * (size), gfp)
#define krealloc_array(p, n, size, gfp) krealloc(p, (n) * (size), gfp)
#define kvmalloc_array(n, size, gfp) kmalloc((n) * (size), gfp)
#define kvfree(p) kfree(p)
#define vzalloc(size) kzalloc(size, GFP_KERNEL)
#define vmalloc(size) kmalloc(size, GFP_KERNEL)
#define vfree(p) kfree(p)
#define kmalloc_node(size, gfp, node) kmalloc(size, gfp)
#define vzalloc_node(size, node) vzalloc(size)
#define kstrdup_const(s, gfp) (s)
#define kfree_const(s) do { } while (0)
#define DEFINE_FREE(...)
#define __free(...)

struct kmem_cache;
struct list_lru;
struct kmem_cache *kmem_cache_create_simple(const char *name, size_t size);
void *kmem_cache_alloc(struct kmem_cache *cache, gfp_t gfp);
void kmem_cache_free(struct kmem_cache *cache, void *p);
#define kmem_cache_alloc_lru(cache, lru, gfp) kmem_cache_alloc(cache, gfp)
#define kmem_cache_shrink(cache) do { } while (0)

#endif
