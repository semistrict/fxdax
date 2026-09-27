//! Generates the syscalls the in-process kernel does not provide.
//!
//! Every syscall declared in the vendored `zx-sys` definitions must resolve,
//! since the `zx` crate refers to all of them. Those not defined under `src/`
//! get a definition that returns `ZX_ERR_NOT_SUPPORTED`, or panics if the
//! syscall does not return a status.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const DEFINITIONS: &str = "../../vendor/fuchsia/sdk/rust/zx-sys/src/definitions.rs";

fn main() {
    println!("cargo::rerun-if-changed={DEFINITIONS}");
    println!("cargo::rerun-if-changed=src");
    let definitions = std::fs::read_to_string(DEFINITIONS).expect("read definitions.rs");
    let mut implemented = BTreeSet::new();
    find_implemented(Path::new("src"), &mut implemented);

    let mut out = String::new();
    let mut declared = BTreeSet::new();
    for signature in declarations(&definitions) {
        let name = signature[..signature.find('(').unwrap()].trim().to_string();
        declared.insert(name.clone());
        if implemented.contains(&name) {
            continue;
        }
        let returns = signature.rsplit_once("->").map(|(_, ret)| ret.trim());
        let body = match returns {
            Some("zx_status_t") => "ZX_ERR_NOT_SUPPORTED".to_string(),
            _ => format!("super::unsupported(\"{name}\")"),
        };
        writeln!(out, "#[unsafe(no_mangle)]\npub unsafe extern \"C\" fn {signature} {{ {body} }}\n")
            .unwrap();
    }
    for name in implemented.difference(&declared) {
        panic!("src defines {name}, which zx-sys does not declare");
    }
    let path = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("unsupported.rs");
    std::fs::write(path, out).expect("write unsupported.rs");
}

/// Returns each `pub fn` in the `extern "C"` block, without `pub fn` and `;`.
fn declarations(definitions: &str) -> Vec<String> {
    let block = &definitions[definitions.find("unsafe extern \"C\" {").expect("extern block")..];
    block
        .split("pub fn ")
        .skip(1)
        .map(|item| {
            let signature = &item[..item.find(';').expect("declaration ends with ;")];
            signature.split_whitespace().collect::<Vec<_>>().join(" ").replace("( ", "(")
        })
        .collect()
}

/// Collects the names of syscalls defined with `extern "C" fn zx_...` under `dir`.
fn find_implemented(dir: &Path, implemented: &mut BTreeSet<String>) {
    const MARKER: &str = "extern \"C\" fn ";
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.unwrap().path();
        if path.is_dir() {
            find_implemented(&path, implemented);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            for (index, _) in source.match_indices(MARKER) {
                let rest = &source[index + MARKER.len()..];
                let name = &rest[..rest.find('(').unwrap()];
                if name.starts_with("zx_") {
                    implemented.insert(name.to_string());
                }
            }
        }
    }
}
