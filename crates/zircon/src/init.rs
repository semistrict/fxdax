//! Starts the kernel's background services with the PMM, standing in for the init hooks Zircon
//! runs as it boots (`LK_INIT_HOOK`) and the executor's initialization.

use crate::object::memory_watchdog;
use crate::vm::pmm::PmmNode;
use crate::vm::scanner::scanner_init;

pub(crate) fn init_hooks(pmm: &'static PmmNode) {
    // The scanner starts at LK_INIT_LEVEL_USER - 1, before the executor.
    scanner_init(pmm);
    // The executor starts the memory watchdog as it initializes.
    memory_watchdog::init(pmm);
}
