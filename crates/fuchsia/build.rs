// Fuchsia's build sets the API level the SDK targets; the vendored library
// selects its diagnostics types with it.
fn main() {
    println!("cargo:rustc-cfg=fuchsia_api_level_at_least=\"27\"");
}
