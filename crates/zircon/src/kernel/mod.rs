//! Kernel primitives the VM relies on: time, deadlines, events and the current thread's faults.
//!
//! Zircon implements these on its scheduler. Here they are built from host
//! threads, so a blocked kernel operation blocks the calling host thread.

pub mod deadline;
pub mod event;
pub mod range_check;
pub mod thread;
pub mod timer;
pub mod types;
