# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
#
# fs/fxfs's dax.c in a process of its own (README.md): built on Linux, with clang, against a
# configured tree of the guest's kernel for what is taken from it unchanged.
#
#   make KERNEL=<the kernel's tree> [BUILD=<where>]

KERNEL ?= $(HOME)/fxdax-guest-kernel/kernel6.18-6.18.54-121.166.amzn2023-arm64
BUILD ?= build
FXFS := ../linux/fs/fxfs
CC := clang
# COV=1 has llvm-cov say what of the real sources a run reached.
COVERAGE := $(if $(filter 1,$(COV)),-fprofile-instr-generate -fcoverage-mapping)

# The kernel's own sources, compiled as they are.
REAL_SOURCES := fs/dax.c fs/iomap/iter.c lib/xarray.c lib/genalloc.c
# The kernel's own headers, used as they are; the shim's take the rest.
REAL_HEADERS := linux/kconfig.h linux/list.h linux/poison.h linux/hash.h linux/hashtable.h \
	linux/rculist.h linux/xarray.h linux/iomap.h linux/dax.h linux/genalloc.h linux/wait.h \
	linux/wait_bit.h trace/events/fs_dax.h

CFLAGS := -std=gnu11 -g -O1 -nostdinc -D__KERNEL__ -DKBUILD_MODNAME='"fxfs"' \
	-isystem $(shell $(CC) -print-resource-dir)/include \
	-I$(BUILD)/alias -Ishim -I$(BUILD)/real -I../linux/include \
	-include $(BUILD)/real/linux/kconfig.h \
	-Wall -Wno-unused-function -Wno-unused-variable -Wno-pointer-sign -Wno-format \
	-Wno-frame-address -Wno-gnu-variable-sized-type-not-at-end \
	-fno-strict-aliasing -fno-omit-frame-pointer -fsanitize=address,undefined $(COVERAGE)
HOSTFLAGS := -std=gnu11 -g -O1 -Wall -fno-omit-frame-pointer -fsanitize=address,undefined
LDFLAGS := -fsanitize=address,undefined -fuse-ld=lld -Wl,--no-demangle $(COVERAGE)

ALIASES := $(shell sed -e '/^\#/d' -e 's/^[^:]*://' shim/ALIASES)

.PHONY: all headers compile flags clean
.SECONDARY:
all: compile

headers: $(BUILD)/.headers
$(BUILD)/.headers: shim/ALIASES Makefile
	rm -f $(BUILD)/.headers
	for header in $(REAL_HEADERS); do \
		mkdir -p $(BUILD)/real/$$(dirname $$header); \
		ln -sf $(abspath $(KERNEL))/include/$$header $(BUILD)/real/$$header; \
	done
	sed -e '/^#/d' shim/ALIASES | while IFS=: read -r target headers; do \
		for header in $$headers; do \
			mkdir -p $(BUILD)/alias/$$(dirname $$header); \
			echo "#include <$$target>" > $(BUILD)/alias/$$header; \
		done; \
	done
	touch $@

# What is real, and what is the fake beneath it.
REAL_OBJECTS := $(BUILD)/fxfs/dax.o $(BUILD)/fxfs/pages.o $(BUILD)/fxfs/data.o \
	$(BUILD)/fxfs/device.o \
	$(patsubst %.c,$(BUILD)/kernel-real/%.o,$(REAL_SOURCES))
FAKE_SOURCES := $(wildcard kernel/*.c fxfs/*.c)
FAKE_OBJECTS := $(patsubst %.c,$(BUILD)/fake/%.o,$(FAKE_SOURCES))
OBJECTS := $(REAL_OBJECTS) $(FAKE_OBJECTS)
TESTS := $(patsubst tests/%.c,$(BUILD)/tests/%,$(wildcard tests/*.c))

compile: $(TESTS)

$(BUILD)/fxfs/%.o: $(FXFS)/%.c $(BUILD)/.headers $(wildcard shim/sim/*.h)
	mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/kernel-real/%.o: $(KERNEL)/%.c $(BUILD)/.headers $(wildcard shim/sim/*.h)
	mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

# The one file that sees libc rather than the shim.
$(BUILD)/fake/kernel/host.o: kernel/host.c
	mkdir -p $(dir $@)
	$(CC) $(HOSTFLAGS) -c $< -o $@

$(BUILD)/fake/%.o: %.c $(BUILD)/.headers $(wildcard shim/sim/*.h)
	mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/tests/%.o: tests/%.c $(BUILD)/.headers $(wildcard shim/sim/*.h tests/*.h)
	mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

# Whatever the real sources and tests name that nothing here defines, as the linker says: each
# stopping a run that reaches it, so that this file is the list of what isn't simulated.
$(BUILD)/stubs.c: $(OBJECTS) $(patsubst %,%.o,$(TESTS))
	$(CC) $(LDFLAGS) -Wl,--error-limit=0 $(OBJECTS) $(firstword $(patsubst %,%.o,$(TESTS))) -o $(BUILD)/stubs.out 2>&1 | \
		sed -n "s/.*undefined symbol: //p" | sort -u | \
		sed -e 's/.*/void &(void) { sim_panic("not simulated: &"); }/' \
		    -e '1i #include <sim/base.h>' > $@

$(BUILD)/stubs.o: $(BUILD)/stubs.c
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/tests/%: $(BUILD)/tests/%.o $(OBJECTS) $(BUILD)/stubs.o
	$(CC) $(LDFLAGS) $^ -o $@

# For mutants.sh: how to build a source here, and what to link it with.
flags:
	@echo "CC='$(CC)'"
	@echo "CFLAGS='$(CFLAGS) -iquote $(abspath $(FXFS))'"
	@echo "LDFLAGS='$(LDFLAGS)'"
	@echo "OBJECTS='$(filter-out $(BUILD)/fxfs/dax.o $(BUILD)/fxfs/pages.o $(BUILD)/fxfs/data.o $(BUILD)/fxfs/device.o $(BUILD)/kernel-real/fs/dax.o,$(OBJECTS)) $(BUILD)/stubs.o'"
	@echo "KERNEL='$(KERNEL)'"
	@echo "BUILD='$(BUILD)'"

clean:
	rm -f $(OBJECTS) $(TESTS) $(BUILD)/.headers $(BUILD)/stubs.*
