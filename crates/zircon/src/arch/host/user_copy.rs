//! Copies between kernel and user memory that recover from faults on the user memory.
//!
//! Ported from the interface of `zircon/kernel/include/arch/user_copy.h`, as the Rust
//! `zircon/kernel/arch/riscv64/src/user_copy.rs` implements it, with each architecture's copy
//! routine replaced by `user_copy`.
//!
//! While a copy runs, the thread's `data_fault_resume` tells the fault handler (`exceptions`) to
//! resume a faulting copy at `user_copy_fault` rather than fail the thread, as Zircon's
//! `data_fault_resume` does. A copy that captures faults resumes without resolving the fault and
//! reports it, so that its caller can drop its locks before resolving it. Any other copy resumes
//! only if the fault cannot be resolved.
//!
//! Deviations:
//! - Kernel and user memory share the host process's address space, so there is no range of user
//!   addresses to check pointers against. A fault on an address outside every address space is
//!   reported as for an address that nothing is mapped at.
//! - No `CopyContext`: copies are made where blocking is allowed.

use std::cell::Cell;
use std::ffi::c_void;
use zx_status::Status;

/// A fault captured by a user copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultInfo {
    pub pf_va: usize,
    pub pf_flags: u32,
}

/// The failure of a user copy that captures faults, with the fault if one was captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserCopyCaptureFaultsError {
    pub status: Status,
    pub fault_info: Option<FaultInfo>,
}

/// How the fault handler treats a fault on the current thread, standing in for the bits of
/// Zircon's `data_fault_resume`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DataFaultResume {
    /// Not copying, so faults are not expected.
    None,
    /// Run the page fault handler, and resume the copy to fail only if it cannot resolve the
    /// fault.
    RunFaultHandler,
    /// Capture faults for the caller to resolve, except that access faults are resolved if
    /// `run_access_fault_handler`.
    CaptureFaults { run_access_fault_handler: bool },
}

thread_local! {
    static DATA_FAULT_RESUME: Cell<DataFaultResume> = const { Cell::new(DataFaultResume::None) };
    /// The fault a copy resumed from, set by the fault handler.
    static FAULT: Cell<Option<UserCopyCaptureFaultsError>> = const { Cell::new(None) };
}

/// Returns how the fault handler should treat a fault on the current thread.
pub(super) fn data_fault_resume() -> DataFaultResume {
    DATA_FAULT_RESUME.get()
}

/// Resumes the current thread's copy at `user_copy_fault`, failing it with `error`.
pub(super) fn resume_copy(error: UserCopyCaptureFaultsError) -> usize {
    FAULT.set(Some(error));
    user_copy_fault as *const () as usize
}

/// Copies `len` bytes from `src` to `dst`, returning zero. Its loads and stores are the only
/// accesses of user memory in a copy, and it neither saves registers nor uses the stack, so that
/// a fault at any of them can resume at `user_copy_fault` to return from it.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn user_copy(dst: *mut c_void, src: *const c_void, len: usize) -> usize {
    core::arch::naked_asm!(
        "cbz x2, 3f",
        "cmp x2, #16",
        "b.lo 2f",
        // Copy 16 bytes at a time.
        "1:",
        "ldp x3, x4, [x1], #16",
        "stp x3, x4, [x0], #16",
        "sub x2, x2, #16",
        "cmp x2, #16",
        "b.hs 1b",
        "cbz x2, 3f",
        // Copy the remaining bytes.
        "2:",
        "ldrb w3, [x1], #1",
        "strb w3, [x0], #1",
        "subs x2, x2, #1",
        "b.ne 2b",
        "3:",
        "mov x0, #0",
        "ret",
    )
}

/// Where a copy resumes after a fault, returning one from `user_copy`.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn user_copy_fault() -> usize {
    core::arch::naked_asm!("mov x0, #1", "ret")
}

/// Copies `len` bytes from `src` to `dst`, returning zero. Its loads and stores are the only
/// accesses of user memory in a copy, and it neither saves registers nor uses the stack, so that
/// a fault at any of them can resume at `user_copy_fault` to return from it.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn user_copy(dst: *mut c_void, src: *const c_void, len: usize) -> usize {
    core::arch::naked_asm!("mov rcx, rdx", "rep movsb", "xor eax, eax", "ret")
}

/// Where a copy resumes after a fault, returning one from `user_copy`.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn user_copy_fault() -> usize {
    core::arch::naked_asm!("mov eax, 1", "ret")
}

/// Copies `len` bytes from `src` to `dst`, treating faults as `resume` says.
///
/// # Safety
///
/// The kernel side of the copy must be valid for `len` bytes.
unsafe fn copy(
    dst: *mut c_void,
    src: *const c_void,
    len: usize,
    resume: DataFaultResume,
) -> Result<(), UserCopyCaptureFaultsError> {
    // Faults on the user side are resumed by the fault handler.
    super::exceptions::install();
    let previous = DATA_FAULT_RESUME.replace(resume);
    debug_assert_eq!(previous, DataFaultResume::None, "nested user copy");
    // SAFETY: Guaranteed by the caller for the kernel side, and a fault on the user side resumes
    // at `user_copy_fault`.
    let faulted = unsafe { user_copy(dst, src, len) } != 0;
    DATA_FAULT_RESUME.set(DataFaultResume::None);
    if faulted {
        return Err(FAULT.take().expect("fault handler records the fault"));
    }
    Ok(())
}

/// Copy memory from user space (`src`) to kernel space (`dst`).
///
/// # Safety
///
/// `dst` must be valid for writes of `len` bytes.
pub unsafe fn arch_copy_from_user(
    dst: *mut c_void,
    src: *const c_void,
    len: usize,
) -> Result<(), Status> {
    // SAFETY: Guaranteed by the caller.
    unsafe { copy(dst, src, len, DataFaultResume::RunFaultHandler) }.map_err(|error| error.status)
}

/// Copy memory from kernel space (`src`) to user space (`dst`).
///
/// # Safety
///
/// `src` must be valid for reads of `len` bytes.
pub unsafe fn arch_copy_to_user(
    dst: *mut c_void,
    src: *const c_void,
    len: usize,
) -> Result<(), Status> {
    // SAFETY: Guaranteed by the caller.
    unsafe { copy(dst, src, len, DataFaultResume::RunFaultHandler) }.map_err(|error| error.status)
}

/// Copies `len` bytes from user memory at `src` into kernel memory at `dst`, capturing any page
/// faults.
///
/// # Safety
///
/// `dst` must be valid for writes of `len` bytes.
pub unsafe fn arch_copy_from_user_capture_faults(
    dst: *mut c_void,
    src: *const c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    let resume = DataFaultResume::CaptureFaults { run_access_fault_handler: true };
    // SAFETY: Guaranteed by the caller.
    unsafe { copy(dst, src, len, resume) }
}

/// Copies `len` bytes from kernel memory at `src` into user memory at `dst`, capturing any page
/// faults.
///
/// # Safety
///
/// `src` must be valid for reads of `len` bytes.
pub unsafe fn arch_copy_to_user_capture_faults(
    dst: *mut c_void,
    src: *const c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    let resume = DataFaultResume::CaptureFaults { run_access_fault_handler: true };
    // SAFETY: Guaranteed by the caller.
    unsafe { copy(dst, src, len, resume) }
}
