//! The subset of BoringSSL that fuchsia-merkle uses, implemented with the
//! `sha2` crate: the `bssl_crypto::digest::Sha256` API and the C `SHA256_*`
//! functions it calls directly on targets without the ARMv8 SHA extension.

pub mod digest {
    use sha2::Digest as _;

    /// SHA-256, as `bssl_crypto::digest::Sha256`.
    #[derive(Clone, Default)]
    pub struct Sha256(sha2::Sha256);

    impl Sha256 {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn update(&mut self, data: &[u8]) {
            self.0.update(data);
        }

        pub fn digest(self) -> [u8; 32] {
            self.0.finalize().into()
        }
    }
}

/// `SHA256_CTX` as BoringSSL lays it out: callers allocate it (4-byte
/// aligned) and only hand it back to these functions, so its first bytes hold
/// a pointer to the real hasher, accessed unaligned.
#[repr(C)]
pub struct Sha256Ctx {
    h: [u32; 8],
    nl: u32,
    nh: u32,
    data: [u8; 64],
    num: core::ffi::c_uint,
    md_len: core::ffi::c_uint,
}

fn hasher_slot(ctx: *mut Sha256Ctx) -> *mut *mut digest::Sha256 {
    ctx.cast()
}

/// # Safety
/// `ctx` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SHA256_Init(ctx: *mut Sha256Ctx) -> core::ffi::c_int {
    let hasher = Box::into_raw(Box::new(digest::Sha256::new()));
    // SAFETY: the caller provides writable storage for a context.
    unsafe { hasher_slot(ctx).write_unaligned(hasher) };
    1
}

/// # Safety
/// `ctx` must have been initialized by `SHA256_Init`, and `data` must be
/// valid for reads of `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SHA256_Update(
    ctx: *mut Sha256Ctx,
    data: *const core::ffi::c_void,
    len: usize,
) -> core::ffi::c_int {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let hasher = hasher_slot(ctx).read_unaligned();
        (*hasher).update(core::slice::from_raw_parts(data.cast::<u8>(), len));
    }
    1
}

/// # Safety
/// `ctx` must have been initialized by `SHA256_Init`, and `out` must be valid
/// for writes of 32 bytes. The context is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SHA256_Final(out: *mut u8, ctx: *mut Sha256Ctx) -> core::ffi::c_int {
    // SAFETY: guaranteed by the caller; the hasher was boxed by SHA256_Init.
    unsafe {
        let hasher = Box::from_raw(hasher_slot(ctx).read_unaligned());
        core::ptr::copy_nonoverlapping(hasher.digest().as_ptr(), out, 32);
    }
    1
}

const _: () = assert!(core::mem::size_of::<Sha256Ctx>() >= core::mem::size_of::<usize>());
