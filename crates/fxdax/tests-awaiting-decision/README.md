# Tests awaiting a decision (TASK-96.1)

Tests of behaviour only the Zircon kernel port, or host Fxfs over it (`fxfs-platform`), had, set
aside verbatim as the port is deleted, for the user to decide on. Nothing here is compiled.

- `zircon_only.rs`: single tests, each with where it came from and why it no longer applies.
- `memory_pressure.rs`: was `crates/fxdax/tests/memory_pressure.rs`: a volume writes back dirty
  pages under the kernel's critical memory pressure. Host Fxfs keeps no dirty pages now.
- `shared_pages.rs`: was `crates/fxdax/tests/shared_pages.rs` (see `zircon_only.rs`).
- `filesystem_unit_tests.rs`: the old unit tests of `crates/fxdax/src/filesystem.rs`, those of
  fxfs-platform's scope, pager and streams, kept whole; those whose behaviour survives are ported in
  `src/filesystem.rs` and `src/volume.rs`.
