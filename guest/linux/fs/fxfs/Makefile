# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
# Copyright 2026 The fxdax Authors.
obj-$(CONFIG_FXFS_FS) += fxfs.o
fxfs-y := rt.o super.o data.o device.o fxfs_rust.o
fxfs-$(CONFIG_FS_DAX) += dax.o pages.o

# fxdax: what a kernel with GCOV_KERNEL counts (FXDAX_GUEST_KERNEL_COVERAGE, TASK-119.14).
GCOV_PROFILE_dax.o := y
GCOV_PROFILE_data.o := y
GCOV_PROFILE_device.o := y
GCOV_PROFILE_pages.o := y
GCOV_PROFILE_super.o := y

# fxdax: the commit the build is of, which /proc/fs/fxfs/build shows: scripts/build-guest-kernel.sh
# gives it, as scripts/build-commit.sh says (TASK-111.7).
FXFS_BUILD_COMMIT ?= unknown
CFLAGS_super.o += -DFXFS_BUILD_COMMIT='"$(FXFS_BUILD_COMMIT)"'

# The filesystem's Rust, which cargo builds (see Kconfig), as one relocatable object: the members
# of the archive the kernel's calls reach. Link-time optimization makes the filesystem's code one
# member, holding every function fxfs.h declares, which fxfs_init names; compiler intrinsics it
# calls come with it, without the LLVM bitcode they carry.
#
# On x86-64, objtool lists the object's returns and indirect calls, as it does for the kernel's
# own objects, so that the kernel patches them at boot as it does its own: unlisted, each return
# goes through the default return thunk, which only warns that it should not be in use.
rust-objtool-args-$(CONFIG_MITIGATION_RETPOLINE) += --retpoline
rust-objtool-args-$(CONFIG_MITIGATION_RETHUNK) += --rethunk
quiet_cmd_rust_archive = LD      $@
      cmd_rust_archive = $(LD) $(KBUILD_LDFLAGS) -r -o $@ -u fxfs_init $< && \
		$(OBJCOPY) --remove-section=.llvmbc --remove-section=.llvmcmd $@ \
		$(if $(rust-objtool-args-y), && $(objtool) $(rust-objtool-args-y) --link $@)
$(obj)/fxfs_rust.o: $(src)/libfxfs_linux.a FORCE
	$(call if_changed,rust_archive)
targets += fxfs_rust.o
