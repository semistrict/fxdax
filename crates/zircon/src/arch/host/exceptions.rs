// Copyright 2016 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Page faults taken by the host process on the address spaces it maps.
//!
//! This plays the part of the architecture's exception handling, ported from
//! `arm64_data_abort_handler` in `zircon/kernel/arch/arm64/exceptions_c.cc`, which decodes a data
//! abort and hands it to `vmm_page_fault_handler`, unless a user copy captures it. Here the host
//! delivers faults as `SIGSEGV` or `SIGBUS`.
//!
//! A fault in an address space is resolved and the faulting access retried, which may block the
//! faulting thread waiting for a pager, as it would block a Zircon thread. One that cannot be
//! resolved fails a user copy, and is otherwise delivered to the faulting thread's exception
//! channel if it has one, or fatal to the process. Faults elsewhere, outside user copies, are passed
//! to the handler that was installed before, which for the Rust runtime reports stack overflows and
//! otherwise lets the fault kill the process.

use super::user_copy::{
    DataFaultResume, FaultInfo, UserCopyCaptureFaultsError, data_fault_resume, resume_copy,
};
use crate::debug::ltracef;
use crate::object::thread_dispatcher::{ExceptionReport, deliver_fatal_page_fault};
use crate::vm::fault::flag;
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vmm::vmm_page_fault_handler;
use std::sync::Once;
use zx_status::Status;

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
                // Under Shuttle (TASK-44.3), threads are coroutines of one host thread, which a
                // handler waiting for a lock switches between: their faults are handled meanwhile,
                // where a fault with the signal blocked would kill the process (Linux).
                #[cfg(feature = "shuttle")]
                {
                    action.sa_flags |= libc::SA_NODEFER;
                }
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
    let aspace = VmAspace::find_host_aspace(addr);
    let dfr = data_fault_resume();
    if aspace.is_none() && dfr == DataFaultResume::None {
        // Not a fault of the kernel's.
        // SAFETY: Passing on the fault as the host would have delivered it.
        unsafe { chain(signal, info, context) };
        return;
    }
    let mut pf_flags = match &aspace {
        Some(aspace) => aspace.host_fault_flags(addr, write),
        // Nothing is mapped outside the address spaces.
        None => flag::NOT_PRESENT | if write { flag::WRITE } else { 0 },
    };
    // An access by a user copy is the kernel's, and any other is the user's.
    let is_user = dfr == DataFaultResume::None;
    if is_user {
        pf_flags |= flag::USER;
    }
    let is_access = pf_flags & flag::ACCESS != 0;

    // Check if we want to capture this fault.
    let capture_fault = match dfr {
        DataFaultResume::CaptureFaults { run_access_fault_handler } => {
            // Only capture this fault if it is _not_ an access fault that should be handled.
            !(run_access_fault_handler && is_access)
        }
        DataFaultResume::None | DataFaultResume::RunFaultHandler => false,
    };

    if capture_fault {
        let fault_info = FaultInfo { pf_va: addr, pf_flags };
        let error = UserCopyCaptureFaultsError {
            status: Status::INVALID_ARGS,
            fault_info: Some(fault_info),
        };
        // SAFETY: The context is of a fault in a user copy.
        unsafe { set_pc(context, resume_copy(error)) };
        return;
    }

    let result = vmm_page_fault_handler(addr, pf_flags);
    ltracef!("addr {addr:#x} flags {pf_flags:#x}: {result:?}\n");
    let Err(synth_code) = result else {
        // The fault was resolved, so returning retries the access.
        return;
    };

    // Check if the current thread was expecting a data fault and we should return to its handler.
    if dfr != DataFaultResume::None {
        let error = UserCopyCaptureFaultsError { status: Status::INVALID_ARGS, fault_info: None };
        // SAFETY: The context is of a fault in a user copy.
        unsafe { set_pc(context, resume_copy(error)) };
        return;
    }

    // Report the fault to the thread's exception channel, if it has one.
    deliver_fatal_page_fault(ExceptionReport { fault_address: addr, synth_code });
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
    uc_mcontext: *mut DarwinMcontext64,
}

/// The start of `struct __darwin_mcontext64`, from `<mach/arm/_structs.h>`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[repr(C)]
struct DarwinMcontext64 {
    es: DarwinArmExceptionState64,
    ss: DarwinArmThreadState64,
}

/// `struct __darwin_arm_exception_state64`, from `<mach/arm/_structs.h>`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[repr(C)]
struct DarwinArmExceptionState64 {
    far: u64,
    esr: u32,
    exception: u32,
}

/// `struct __darwin_arm_thread_state64`, from `<mach/arm/_structs.h>`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[repr(C)]
struct DarwinArmThreadState64 {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    flags: u32,
}

/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn is_write_fault(context: *mut libc::c_void) -> bool {
    // SAFETY: Guaranteed by the caller.
    let esr = unsafe { (*(*context.cast::<DarwinUcontext>()).uc_mcontext).es.esr };
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

/// Sets where the thread whose fault `context` is resumes.
///
/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn set_pc(context: *mut libc::c_void, pc: usize) {
    // SAFETY: Guaranteed by the caller.
    unsafe { (*(*context.cast::<DarwinUcontext>()).uc_mcontext).ss.pc = pc as u64 };
}

/// Sets where the thread whose fault `context` is resumes.
///
/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn set_pc(context: *mut libc::c_void, pc: usize) {
    // SAFETY: Guaranteed by the caller.
    unsafe { (*context.cast::<libc::ucontext_t>()).uc_mcontext.pc = pc as u64 };
}

/// Sets where the thread whose fault `context` is resumes.
///
/// # Safety
///
/// `context` must be the context of a fault.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn set_pc(context: *mut libc::c_void, pc: usize) {
    // SAFETY: Guaranteed by the caller.
    unsafe {
        (*context.cast::<libc::ucontext_t>()).uc_mcontext.gregs[libc::REG_RIP as usize] = pc as i64
    };
}
