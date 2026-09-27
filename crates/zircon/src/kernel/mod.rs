//! Kernel primitives the VM relies on: time, deadlines and events.
//!
//! Zircon implements these on its scheduler. Here they are built from host
//! threads, so a blocked kernel operation blocks the calling host thread.

pub mod deadline;
pub mod event;
pub mod types;
