//! Builds storage_device's Fuchsia code paths, such as VMO-backed buffers, on the in-process
//! kernel. The sources are vendor/fuchsia-kernel's copy (see scripts/kernel-cfg.py).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(fxdax_kernel)");
    println!("cargo::rustc-cfg=fxdax_kernel");
}
