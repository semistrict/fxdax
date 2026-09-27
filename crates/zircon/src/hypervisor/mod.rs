//! The parts of Zircon's hypervisor support that serve guests' memory, ported from
//! `zircon/kernel/hypervisor`.

pub mod aspace;

#[cfg(test)]
mod hypervisor_unittest;
