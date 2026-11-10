// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what the guest kernel does as fxdax configures it (guest/linux/configs/fxdax.config),
//! where its command line doesn't say.

use super::*;

/// An oops panics the kernel (CONFIG_PANIC_ON_OOPS), the command line saying nothing of it: a task
/// that oopsed died holding a lock, which every vCPU then spun on for minutes, the oops long gone
/// from a console's tail (TASK-311).
pub(super) fn test_oops_panics() {
    let command_line = fs::read_to_string("/proc/cmdline").unwrap();
    assert!(!command_line.contains("oops="), "{command_line}");
    assert_eq!(fs::read_to_string("/proc/sys/kernel/panic_on_oops").unwrap(), "1\n");
}
