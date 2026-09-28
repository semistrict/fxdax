//! Builds the copy of paged_object_handle.rs in vendor/fuchsia-kernel, whose tests are compiled
//! out on the in-process kernel (see scripts/kernel-cfg.py).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(fxdax_kernel)");
    println!("cargo::rustc-cfg=fxdax_kernel");
}
