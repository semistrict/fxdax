//! Processes, of which the kernel has none to run or exit.

/// Panics the kernel.
pub fn abort() -> ! {
    panic!("abort")
}

/// Panics the kernel, which a filesystem cannot exit.
pub fn exit(code: i32) -> ! {
    panic!("exit({code})")
}
