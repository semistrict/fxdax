/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * What the kernel's C rests on: its types, compiler attributes and arithmetic. Marked accesses
 * (READ_ONCE, WRITE_ONCE) are plain here: one simulated task runs at a time.
 */
#ifndef _SIM_BASE_H
#define _SIM_BASE_H

#include <stdarg.h>

#include <sim/errno.h>

#define NULL ((void *)0)
#define offsetof(type, member) __builtin_offsetof(type, member)
#define offsetofend(type, member) (offsetof(type, member) + sizeof(((type *)0)->member))
#define sizeof_field(type, member) sizeof(((type *)0)->member)

typedef signed char s8;
typedef unsigned char u8;
typedef short s16;
typedef unsigned short u16;
typedef int s32;
typedef unsigned int u32;
typedef long long s64;
typedef unsigned long long u64;
typedef u8 __u8;
typedef u16 __u16;
typedef u32 __u32;
typedef u64 __u64;
typedef s32 __s32;
typedef s64 __s64;
typedef u16 __le16;
typedef u32 __le32;
typedef u64 __le64;
typedef u64 uint64_t;
typedef u32 uint32_t;
typedef unsigned long uintptr_t;
typedef __SIZE_TYPE__ size_t;
typedef long ssize_t;
typedef long long loff_t;
typedef unsigned long pgoff_t;
typedef u64 sector_t;
typedef u64 blkcnt_t;
typedef u64 phys_addr_t;
typedef u32 dma_addr_t;
typedef unsigned int gfp_t;
typedef unsigned int fmode_t;
typedef unsigned short umode_t;
typedef unsigned int vm_fault_t;
typedef long long ktime_t;
typedef _Bool bool;

enum { false = 0, true = 1 };

typedef struct { int counter; } atomic_t;
typedef struct { s64 counter; } atomic64_t;
typedef struct { long counter; } atomic_long_t;
typedef struct { atomic_t refs; } refcount_t;

struct list_head {
	struct list_head *next, *prev;
};
struct hlist_head {
	struct hlist_node *first;
};
struct hlist_node {
	struct hlist_node *next, **pprev;
};
struct callback_head {
	struct callback_head *next;
	void (*func)(struct callback_head *head);
};
#define rcu_head callback_head

#define __force
#define __user
#define __iomem
#define __rcu
#define __percpu
#define __bitwise
#define __private
#define __init
#define __exit
#define __ref
#define __cold
#define __pure __attribute__((pure))
#define __must_check __attribute__((warn_unused_result))
#define __maybe_unused __attribute__((unused))
#define __always_unused __attribute__((unused))
#define __always_inline inline __attribute__((always_inline))
#define __packed __attribute__((packed))
#define __aligned(x) __attribute__((aligned(x)))
#define __printf(a, b) __attribute__((format(printf, a, b)))
#define __weak __attribute__((weak))
#define __noreturn __attribute__((noreturn))
#define __counted_by(member)
#define __nonstring
#define __cacheline_aligned
#define ____cacheline_aligned_in_smp
#define __read_mostly
#define __ro_after_init
#define __refdata
#define __nocast
#define __acquires(x)
#define __releases(x)
#define __must_hold(x)
#define __sched
#define __randomize_layout
#define __no_randomize_layout
#define __section(name) __attribute__((section(name)))
#define __used __attribute__((used))
#define notrace
#define noinline __attribute__((noinline))
#define noinstr
#define fallthrough __attribute__((__fallthrough__))
#define asmlinkage
#define __builtin_expect_nop(x) (x)

#define likely(x) __builtin_expect(!!(x), 1)
#define unlikely(x) __builtin_expect(!!(x), 0)
#define barrier() __asm__ __volatile__("" : : : "memory")
#define smp_mb() barrier()
#define smp_rmb() barrier()
#define smp_wmb() barrier()
#define smp_mb__before_atomic() barrier()
#define smp_mb__after_atomic() barrier()
#define smp_store_release(p, v) WRITE_ONCE(*(p), v)
#define smp_load_acquire(p) READ_ONCE(*(p))
#define smp_store_mb(var, value) ((void)((var) = (value)))
#define cpu_relax() barrier()

#define READ_ONCE(x) (*(const volatile typeof(x) *)&(x))
#define WRITE_ONCE(x, val) ((void)(*(volatile typeof(x) *)&(x) = (val)))
#define data_race(expr) ({ typeof(({ expr; })) __v = ({ expr; }); __v; })
#define ASSERT_EXCLUSIVE_WRITER(x)
#define ASSERT_EXCLUSIVE_ACCESS(x)
#define kcsan_check_access(ptr, size, type)

#define __same_type(a, b) __builtin_types_compatible_p(typeof(a), typeof(b))
#define __is_constexpr(x) __builtin_constant_p(x)
#define __must_be_array(a) 0
#define BUILD_BUG_ON(cond) _Static_assert(!(cond), "BUILD_BUG_ON")
#define BUILD_BUG_ON_ZERO(e) 0
#define BUILD_BUG_ON_MSG(cond, msg) _Static_assert(!(cond), msg)
#define BUILD_BUG() __builtin_unreachable()
#ifndef static_assert
#define static_assert(expr, ...) _Static_assert(expr, #expr)
#endif
#define is_signed_type(type) (((type)(-1)) < (type)1)

#define container_of(ptr, type, member) \
	({ void *__mptr = (void *)(ptr); ((type *)(__mptr - offsetof(type, member))); })
#define container_of_const(ptr, type, member) container_of(ptr, type, member)
#define typeof_member(T, m) typeof(((T *)0)->m)

#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define BITS_PER_BYTE 8
#define BITS_PER_LONG 64
#define BITS_PER_LONG_LONG 64
#define BITS_TO_LONGS(n) DIV_ROUND_UP(n, BITS_PER_LONG)
#define BIT(n) (1UL << (n))
#define BIT_ULL(n) (1ULL << (n))
#define BIT_WORD(n) ((n) / BITS_PER_LONG)
#define BIT_MASK(n) (1UL << ((n) % BITS_PER_LONG))
#define GENMASK(h, l) (((~0UL) << (l)) & (~0UL >> (BITS_PER_LONG - 1 - (h))))
#define UL(x) (x##UL)
#define ULL(x) (x##ULL)
#define __AC(x, y) (x##y)
#define _AC(x, y) __AC(x, y)
#define _UL(x) (x##UL)
#define _ULL(x) (x##ULL)
#define _BITUL(x) (1UL << (x))
#define _BITULL(x) (1ULL << (x))

#define S8_MAX 127
#define U8_MAX 255
#define U16_MAX 65535
#define S32_MAX 2147483647
#define U32_MAX 4294967295U
#define S64_MAX 9223372036854775807LL
#define U64_MAX 18446744073709551615ULL
#define INT_MAX 2147483647
#define INT_MIN (-INT_MAX - 1)
#define UINT_MAX 4294967295U
#define LONG_MAX 9223372036854775807L
#define LONG_MIN (-LONG_MAX - 1)
#define ULONG_MAX (~0UL)
#define LLONG_MAX 9223372036854775807LL
#define LLONG_MIN (-LLONG_MAX - 1)
#define ULLONG_MAX (~0ULL)
#define SIZE_MAX (~(size_t)0)
#define PHYS_ADDR_MAX (~(phys_addr_t)0)
#define MAX_LFS_FILESIZE ((loff_t)LLONG_MAX)
#define PATH_MAX 4096

#define __sim_cmp(a, b, op) ({ typeof(a) __a = (a); typeof(b) __b = (b); __a op __b ? __a : __b; })
#define min(a, b) __sim_cmp(a, b, <)
#define max(a, b) __sim_cmp(a, b, >)
#define umin(a, b) __sim_cmp((u64)(a), (u64)(b), <)
#define umax(a, b) __sim_cmp((u64)(a), (u64)(b), >)
#define min_not_zero(a, b) ({ typeof(a) __a = (a); typeof(b) __b = (b); \
	__a == 0 ? __b : __b == 0 ? __a : min(__a, __b); })
#define min_t(type, a, b) __sim_cmp((type)(a), (type)(b), <)
#define max_t(type, a, b) __sim_cmp((type)(a), (type)(b), >)
#define max3(a, b, c) max(max(a, b), c)
#define min3(a, b, c) min(min(a, b), c)
#define clamp(v, lo, hi) min(max(v, lo), hi)
#define clamp_t(type, v, lo, hi) min_t(type, max_t(type, v, lo), hi)
#define swap(a, b) do { typeof(a) __t = (a); (a) = (b); (b) = __t; } while (0)

#define __ALIGN_MASK(x, mask) (((x) + (mask)) & ~(mask))
#define ALIGN(x, a) __ALIGN_MASK(x, (typeof(x))(a) - 1)
#define ALIGN_DOWN(x, a) ((x) & ~((typeof(x))(a) - 1))
#define IS_ALIGNED(x, a) (((x) & ((typeof(x))(a) - 1)) == 0)
#define PTR_ALIGN(p, a) ((typeof(p))ALIGN((unsigned long)(p), (a)))
#define __round_mask(x, y) ((__typeof__(x))((y) - 1))
#define round_up(x, y) ((((x) - 1) | __round_mask(x, y)) + 1)
#define round_down(x, y) ((x) & ~__round_mask(x, y))
#define rounddown(x, y) ({ typeof(x) __x = (x); __x - (__x % (y)); })
#define roundup(x, y) ({ typeof(y) __y = (y); (((x) + (__y - 1)) / __y) * __y; })
#define DIV_ROUND_UP(n, d) (((n) + (d) - 1) / (d))
#define DIV_ROUND_UP_ULL(n, d) DIV_ROUND_UP((u64)(n), d)
#define do_div(n, base) ({ u32 __rem = (n) % (base); (n) /= (base); __rem; })

/* The kernel's IS_ERR family. */
#define MAX_ERRNO 4095
#define IS_ERR_VALUE(x) ((unsigned long)(void *)(x) >= (unsigned long)-MAX_ERRNO)
#define cpu_to_le32(x) ((u32)(x))
#define cpu_to_le64(x) ((u64)(x))
#define le32_to_cpu(x) ((u32)(x))
#define le64_to_cpu(x) ((u64)(x))

static inline void *ERR_PTR(long error) { return (void *)error; }
static inline long PTR_ERR(const void *ptr) { return (long)ptr; }
static inline bool IS_ERR(const void *ptr) { return IS_ERR_VALUE((unsigned long)ptr); }
static inline bool IS_ERR_OR_NULL(const void *ptr) { return !ptr || IS_ERR(ptr); }
static inline void *ERR_CAST(const void *ptr) { return (void *)ptr; }

/* String functions, the C library's. */
void *memcpy(void *to, const void *from, size_t n);
void *memmove(void *to, const void *from, size_t n);
void *memset(void *to, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
char *strcpy(char *to, const char *from);
int sscanf(const char *s, const char *format, ...) __attribute__((format(scanf, 2, 3)));
int snprintf(char *s, size_t n, const char *format, ...) __printf(3, 4);
int vsnprintf(char *s, size_t n, const char *format, va_list args);
unsigned long simple_strtoul(const char *s, char **end, unsigned int base);

/* Printing, and what kills a run. */
__printf(1, 2) void sim_log(const char *format, ...);
__printf(1, 2) __noreturn void sim_panic(const char *format, ...);
/* A WARN fails a run, as the runner fails a test whose kernel WARNs (TASK-44.8). */
__printf(3, 4) void sim_warn(const char *file, int line, const char *format, ...);

#define KERN_ERR ""
#define KERN_WARNING ""
#define KERN_INFO ""
#define KERN_DEFAULT ""
#define pr_fmt(fmt) fmt
#define pr_err(...) sim_log(__VA_ARGS__)
#define pr_warn(...) sim_log(__VA_ARGS__)
#define pr_info(...) sim_log(__VA_ARGS__)
#define pr_notice(...) sim_log(__VA_ARGS__)
#define pr_debug(...) do { } while (0)
#define pr_err_ratelimited(...) sim_log(__VA_ARGS__)
#define pr_warn_ratelimited(...) sim_log(__VA_ARGS__)
#define pr_warn_once(...) sim_log(__VA_ARGS__)
#define printk(...) sim_log(__VA_ARGS__)

#define WARN(cond, ...) ({ \
	bool __c = !!(cond); \
	if (unlikely(__c)) \
		sim_warn(__FILE__, __LINE__, __VA_ARGS__); \
	unlikely(__c); })
#define WARN_ON(cond) WARN(cond, "%s", #cond)
#define WARN_ONCE(cond, ...) WARN(cond, __VA_ARGS__)
#define WARN_ON_ONCE(cond) WARN_ON(cond)
#define BUG() sim_panic("BUG at %s:%d", __FILE__, __LINE__)
#define BUG_ON(cond) do { if (unlikely(cond)) sim_panic("BUG_ON(%s) at %s:%d", #cond, __FILE__, __LINE__); } while (0)
#define VM_BUG_ON(cond) BUG_ON(cond)
#define VM_WARN_ON_ONCE(cond) WARN_ON(cond)
#define VM_WARN_ON(cond) WARN_ON(cond)

#define _RET_IP_ ((unsigned long)__builtin_return_address(0))

#define EXPORT_SYMBOL(sym)
#define EXPORT_SYMBOL_GPL(sym)
#define EXPORT_SYMBOL_NS_GPL(sym, ns)
#define MODULE_LICENSE(x)
#define MODULE_AUTHOR(x)
#define MODULE_DESCRIPTION(x)
#define __initcall(fn)

/* What lib/genalloc.c asks of the rest of the kernel, in its parts that aren't reached here. */
struct device;
struct device_node;
#define in_nmi() 0
#define dev_of_node(dev) NULL
#define devres_alloc(release, size, gfp) ((void *)NULL)
#define devres_free(res) do { } while (0)
#define devres_add(dev, res) do { } while (0)
#define devres_find(dev, release, match, data) ((void *)NULL)
#define devres_release(dev, release, match, data) (-ENOENT)

/* Static keys: a flag. */
struct static_key_false {
	bool enabled;
};
#define DECLARE_STATIC_KEY_FALSE(name) extern struct static_key_false name
#define DEFINE_STATIC_KEY_FALSE(name) struct static_key_false name
#define static_branch_unlikely(key) unlikely((key)->enabled)

typedef int (*sim_initcall_t)(void);
#define fs_initcall(fn) \
	static sim_initcall_t __sim_init_##fn __used __section("sim_initcalls") = fn

#endif
