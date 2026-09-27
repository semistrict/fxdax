//! Constants shared by the VM's page list and PMM.
//!
//! Ported from `zircon/kernel/vm/include/vm/vm_constants.h`.

#![allow(non_upper_case_globals)]

pub const kPmmNodeIndexZeroBits: u32 = 3;
pub const kVmPageListTypeBits: u32 = 3;
pub const kVmPageListPageType: u32 = 0b000;
pub const kVmPageListReferenceType: u32 = 0b010;
pub const kVmPageListZeroMarkerType: u32 = 0b001;
pub const kVmPageListIntervalType: u32 = 0b011;
pub const kVmPageListParentContentType: u32 = 0b100;
pub const kVmPageListIntervalSentinelBits: u32 = 2;
pub const kVmPageListIntervalTypeBits: u32 = 2;
pub const kVmPageListIntervalBits: u32 =
    kVmPageListTypeBits + kVmPageListIntervalSentinelBits + kVmPageListIntervalTypeBits;
pub const kVmPageListFanOut: usize = 16;
