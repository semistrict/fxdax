//! Builds the vfs library as Fuchsia builds it at API level `HEAD`, without the deprecated
//! fuchsia.io methods.

fn main() {
    for level in ["27", "28", "29", "32", "HEAD"] {
        println!("cargo::rustc-cfg=fuchsia_api_level_at_least=\"{level}\"");
    }
}
