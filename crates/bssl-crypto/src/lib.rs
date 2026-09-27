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

/// `SHA256_CTX` as BoringSSL lays it out. Callers only hand it back to these
/// functions, so it holds a pointer to the real hasher.
#[repr(C)]
pub struct Sha256Ctx {
    state: *mut digest::Sha256,
    _unused: [u8; 104],
}

/// # Safety
/// `ctx` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SHA256_Init(ctx: *mut Sha256Ctx) -> core::ffi::c_int {
    let state = Box::into_raw(Box::new(digest::Sha256::new()));
    // SAFETY: the caller provides writable storage for a context.
    unsafe { (&raw mut (*ctx).state).write(state) };
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
        let data = core::slice::from_raw_parts(data.cast::<u8>(), len);
        (*(*ctx).state).update(data);
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
        let hasher = Box::from_raw((*ctx).state);
        core::ptr::copy_nonoverlapping(hasher.digest().as_ptr(), out, 32);
    }
    1
}
