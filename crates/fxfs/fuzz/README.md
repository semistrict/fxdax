# Fuzzing Fxfs's records

Targets for what a corrupt image can hold (TASK-149.9), each running what `cfg(fuzz)` code in
`crates/fxfs` holds. The `*_records` targets start from the `Arbitrary` derives, so they reach past
checksums; the `*_bytes` targets take raw bytes.

| Target | Input |
| --- | --- |
| `journal_bytes` | bytes in the journal of a filesystem, sealed, then replayed |
| `journal_records` | records in the journal, then replayed |
| `layer_bytes` | a layer file |
| `layer_records` | a good layer of an object store or the allocator, some of its bytes changed |
| `super_block_bytes` | what follows a super-block's magic, sealed |
| `super_block_records` | a filesystem's own super-block, its fields and records changed, sealed |
| `lsm_tree_actions` | what's done to a tree in memory |

They need nightly, and `--cfg fuzz`, which the `Arbitrary` derives are under:

    cargo install cargo-fuzz
    cd crates/fxfs
    RUSTFLAGS="--cfg fuzz" cargo +nightly fuzz run layer_records

A panic, a hang or a large allocation stops one and leaves its input under `fuzz/artifacts`.

cargo-fuzz is `MIT OR Apache-2.0`. libfuzzer-sys is `(MIT OR Apache-2.0) AND NCSA`, and the libFuzzer
it builds says `Apache-2.0 WITH LLVM-exception` in its own sources.
