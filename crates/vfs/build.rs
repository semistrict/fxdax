//! Builds the vendored parts of the vfs library as Fuchsia builds them at API level `HEAD`, without
//! the deprecated fuchsia.io methods, and on the in-process kernel, whose handles its token registry
//! takes the koids of (see scripts/kernel-cfg.py).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(fxdax_kernel)");
    println!("cargo::rustc-cfg=fxdax_kernel");
    for level in ["27", "28", "29", "32", "HEAD"] {
        println!("cargo::rustc-cfg=fuchsia_api_level_at_least=\"{level}\"");
    }
}
