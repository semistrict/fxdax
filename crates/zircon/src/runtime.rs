//! What Fuchsia's process runtime provides a process, from `zircon/process.h`.
//!
//! On Fuchsia libc receives a process's initial handles when it starts. Here the process is the
//! host process, whose address space is created when first asked for.

use crate::boot_options::BootOptions;
use crate::object::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::vm::arch_vm_aspace::ARCH_MMU_FLAG_PERM_USER;
use crate::vm::vm_aspace::VmAspace;
use std::sync::{Arc, OnceLock};
use zx_types::zx_handle_t;

struct Process {
    aspace: Arc<VmAspace>,
    root_vmar: zx_handle_t,
}

fn process() -> &'static Process {
    static PROCESS: OnceLock<Process> = OnceLock::new();
    PROCESS.get_or_init(|| {
        let size = page::round_up(BootOptions::get().user_aspace_bytes as usize);
        let aspace = VmAspace::create_host(size, "process").expect("create the process aspace");
        let root_vmar = aspace.root_vmar().expect("new aspace has a root vmar");
        let (handle, rights) =
            VmAddressRegionDispatcher::create(root_vmar, ARCH_MMU_FLAG_PERM_USER)
                .expect("create the root vmar dispatcher");
        let root_vmar = handle.make_and_add_handle(rights).expect("add the root vmar handle");
        Process { aspace, root_vmar }
    })
}

/// The address space of the process.
pub fn aspace() -> &'static Arc<VmAspace> {
    &process().aspace
}

/// Returns a handle to the root VMAR of the process. The handle is owned by the process runtime
/// and must not be closed.
#[unsafe(no_mangle)]
pub extern "C" fn zx_vmar_root_self() -> zx_handle_t {
    process().root_vmar
}
