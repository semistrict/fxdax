//! The states a physical page can be in.
//!
//! Ported from `zircon/kernel/vm/include/vm/page_state.h`.

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmPageState {
    Free = 0,
    Alloc,
    Object,
    Wired,
    Heap,
    Mmu,
    Iommu,
    Ipc,
    Cache,
    Slab,
    Zram,
    FreeLoaned,
}

impl VmPageState {
    pub const COUNT: usize = 12;

    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Free,
            1 => Self::Alloc,
            2 => Self::Object,
            3 => Self::Wired,
            4 => Self::Heap,
            5 => Self::Mmu,
            6 => Self::Iommu,
            7 => Self::Ipc,
            8 => Self::Cache,
            9 => Self::Slab,
            10 => Self::Zram,
            11 => Self::FreeLoaned,
            _ => panic!("invalid page state"),
        }
    }
}
