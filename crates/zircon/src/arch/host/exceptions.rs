//! Page faults taken by the host process on the address spaces it maps.
//!
//! This plays the part of the architecture's exception handling (for example
//! `zircon/kernel/arch/arm64/exceptions_c.cc`), which decodes a data abort and hands it to
//! `VmAspace::page_fault`. Here the host delivers faults as `SIGSEGV` or `SIGBUS`. A fault in an
//! address space is resolved and the faulting access retried, which may block the faulting thread
//! waiting for a pager, as it would block a Zircon thread. Faults elsewhere, or that cannot be
//! resolved, are passed to the handler that was installed before, which for the Rust runtime
//! reports stack overflows and otherwise lets the fault kill the process.
//!
//! Deviation: a fault that cannot be resolved is not delivered to an exception channel; it is fatal
//! to the process.

use crate::debug::ltracef;
use crate::vm::vm_aspace::VmAspace;
use std::sync::Once;

const LOCAL_TRACE: u32 = 0;

const SIGNALS: [libc::c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];

/// The handlers installed before ours, to pass faults on to.
static mut PREVIOUS: [std::mem::MaybeUninit<libc::sigaction>; 2] =
    [std::mem::MaybeUninit::uninit(), std::mem::MaybeUninit::uninit()];

/// Installs the fault handler, once.
pub fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        for (index, signal) in SIGNALS.iter().enumerate() {
            // SAFETY: The action is fully initialized before use, and PREVIOUS is only written
            // here, before the handler that reads it is installed.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handle_fault as *const () as libc::sighandler_t;
                // Resolving a fault needs more stack than the alternate signal stacks the Rust
                // runtime gives threads, so the handler runs on the faulting thread's stack. A
                // fault from overflowing that stack then cannot be handled, and kills the process
                // without the runtime's report.
                action.sa_flags = libc::SA_SIGINFO;
                libc::sigemptyset(&mut action.sa_mask);
                let previous = (&raw mut PREVIOUS[index]).cast::<libc::sigaction>();
                let status = libc::sigaction(*signal, &action, previous);
                assert_eq!(status, 0, "install fault handler: {}", std::io::Error::last_os_error());
            }
        }
    });
}

extern "C" fn handle_fault(
    signal: libc::c_int,
    info: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    // SAFETY: The host passes a valid siginfo and context for a fault.
    let (addr, write) = unsafe { (fault_address(info), is_write_fault(context)) };
    ltracef!("signal {signal} addr {addr:#x} write {write}\n");
    if let Some(aspace) = VmAspace::find_host_aspace(addr) {
        let result = aspace.host_fault(addr, write);
        ltracef!("addr {addr:#x} write {write}: {result:?}\n");
        if result.is_ok() {
            // The fault was resolved, so returning retries the access.
            return;
        }
    }
    // SAFETY: Passing on the fault as the host would have delivered it.
    unsafe { chain(signal, info, context) };
}

/// Passes a fault to the handler installed before ours.
///
/// # Safety
///
/// The arguments must be those of the signal being handled.
unsafe fn chain(signal: libc::c_int, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    let index = SIGNALS.iter().position(|s| *s == signal).expect("handled signal");
    // SAFETY: PREVIOUS was initialized when the handler was installed.
    let previous = unsafe { &*(&raw const PREVIOUS[index]).cast::<libc::sigaction>() };
    let handler = previous.sa_sigaction;
    if handler == libc::SIG_DFL || handler == libc::SIG_IGN {
        // Restore the default action and return, so the access faults again and the default action
        // is taken.
        // SAFETY: Resetting the signal's action.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
        return;
    }
    // SAFETY: The previous handler was installed for this signal with these flags.
    unsafe {
        if previous.sa_flags & libc::SA_SIGINFO != 0 {
            let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(handler);
            handler(signal, info, context);
        } else {
            let handler: extern "C" fn(libc::c_int) = std::mem::transmute(handler);
            handler(signal);
        }
    }
}

/// # Safety
///
/// `info` must be the siginfo of a fault.
unsafe fn fault_address(info: *mut libc::siginfo_t) -> usize {
    #[cfg(target_os = "linux")]
    // SAFETY: Guaranteed by the caller.
    return unsafe { (*info).si_addr() as usize };
    #[cfg(target_os = "macos")]
    // SAFETY: Guaranteed by the caller.
    return unsafe { (*info).si_addr as usize };
}

/// Returns whether an arm64 exception syndrome describes a write data abort.
#[cfg(target_arch = "aarch64")]
fn esr_is_write(esr: u64) -> bool {
    const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
    const EC_DATA_ABORT_SAME_EL: u64 = 0x25;
    // Write not Read.
    const WNR: u64 = 1 << 6;
    // Cache maintenance, which also sets WnR.
    const CM: u64 = 1 << 8;
    let ec = (esr >> 26) & 0x3f;
    (ec == EC_DATA_ABORT_LOWER_EL || ec == EC_DATA_ABORT_SAME_EL) && esr & WNR != 0 && esr & CM == 0
}

/// The start of Darwin's `struct __darwin_ucontext`, from `<sys/_types/_ucontext.h>`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[repr(C)]
struct DarwinUcontext {
    uc_onstack: libc::c_int,
    uc_sigmask: u32,
    uc_stack: libc::stack_t,
    uc_link: *mut libc::c_void,
    uc_mcsize: usize,
    uc_mcontext: *const DarwinArmExceptionState64,
}

/// `struct __darwin_arm_exception_state64`, from `<mach/arm/_structs.h>`, which starts
/// `struct __darwin_mcontext64`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[repr(C)]
struct DarwinArmExceptionState64 {
    far: u64,
    esr: u32,
    exception: u32,
}

/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn is_write_fault(context: *mut libc::c_void) -> bool {
    // SAFETY: Guaranteed by the caller.
    let esr = unsafe { (*(*context.cast::<DarwinUcontext>()).uc_mcontext).esr };
    esr_is_write(esr.into())
}

/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn is_write_fault(context: *mut libc::c_void) -> bool {
    // The records of `struct sigcontext`'s `__reserved` area, from <asm/sigcontext.h>.
    #[repr(C)]
    struct Aarch64Ctx {
        magic: u32,
        size: u32,
    }
    const ESR_MAGIC: u32 = 0x45535201;
    // SAFETY: Guaranteed by the caller. `__reserved` is 16 byte aligned after `pstate`.
    unsafe {
        let mcontext = &raw const (*context.cast::<libc::ucontext_t>()).uc_mcontext;
        let reserved_offset =
            (std::mem::offset_of!(libc::mcontext_t, pstate) + 8).next_multiple_of(16);
        let mut record = mcontext.cast::<u8>().add(reserved_offset);
        loop {
            let head = &*record.cast::<Aarch64Ctx>();
            if head.magic == 0 || head.size == 0 {
                return false;
            }
            if head.magic == ESR_MAGIC {
                let esr = record.add(size_of::<Aarch64Ctx>()).cast::<u64>().read();
                return esr_is_write(esr);
            }
            record = record.add(head.size as usize);
        }
    }
}

/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn is_write_fault(context: *mut libc::c_void) -> bool {
    // The page fault error code has the write bit set for writes.
    const PF_WRITE: i64 = 1 << 1;
    // SAFETY: Guaranteed by the caller.
    let err =
        unsafe { (*context.cast::<libc::ucontext_t>()).uc_mcontext.gregs[libc::REG_ERR as usize] };
    err & PF_WRITE != 0
}
