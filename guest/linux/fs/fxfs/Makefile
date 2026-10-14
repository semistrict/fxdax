# SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
obj-$(CONFIG_FXFS_FS) += fxfs.o
fxfs-y := rt.o super.o device.o fxfs_rust.o
fxfs-$(CONFIG_FS_DAX) += dax.o

# The filesystem's Rust, which cargo builds (see Kconfig), as one relocatable object: the members
# of the archive the kernel's calls reach. Link-time optimization makes the filesystem's code one
# member, holding every function fxfs.h declares, which fxfs_init names; compiler intrinsics it
# calls come with it, without the LLVM bitcode they carry.
quiet_cmd_rust_archive = LD      $@
      cmd_rust_archive = $(LD) $(KBUILD_LDFLAGS) -r -o $@ -u fxfs_init $< && \
		$(OBJCOPY) --remove-section=.llvmbc --remove-section=.llvmcmd $@
$(obj)/fxfs_rust.o: $(src)/libfxfs_linux.a FORCE
	$(call if_changed,rust_archive)
targets += fxfs_rust.o
