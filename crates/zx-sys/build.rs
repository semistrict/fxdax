//! Declares the Zircon syscalls for the host.
//!
//! The vendored `definitions.rs` only declares its `extern "C"` block when
//! targeting Fuchsia, where the symbols come from the vDSO (`libzircon`). On
//! the host they come from the `zircon` crate, so this copies the file with
//! those two attributes removed and leaves everything else as generated.

use std::path::PathBuf;

const SOURCE: &str = "../../vendor/fuchsia/sdk/rust/zx-sys/src/definitions.rs";
const FUCHSIA_ONLY: [&str; 2] = ["#[cfg(target_os = \"fuchsia\")]", "#[link(name = \"zircon\")]"];

fn main() {
    println!("cargo::rerun-if-changed={SOURCE}");
    let source = std::fs::read_to_string(SOURCE).expect("read definitions.rs");
    let mut removed = 0;
    let host: String = source
        .lines()
        .filter(|line| {
            let fuchsia_only = FUCHSIA_ONLY.contains(&line.trim());
            removed += usize::from(fuchsia_only);
            !fuchsia_only
        })
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(removed, FUCHSIA_ONLY.len(), "definitions.rs no longer has the expected attributes");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("definitions.rs");
    std::fs::write(out, host).expect("write definitions.rs");
}
