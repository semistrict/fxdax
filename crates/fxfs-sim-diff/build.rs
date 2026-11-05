// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Builds guest/sim's model of Fxfs over the shim of guest/sim's kernel (shim/): the model as it
//! is in the tree, or, with FXDAX_SIM_MODEL, a copy of it at that path, as mutants.sh puts a past
//! bug back in. The model's includes relative to its directory find the tree's either way.

use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = crate_dir.join("../..");
    let in_tree = root.join("guest/sim/fxfs/fxfs.c");
    println!("cargo:rerun-if-env-changed=FXDAX_SIM_MODEL");
    let model = std::env::var_os("FXDAX_SIM_MODEL").map_or(in_tree, PathBuf::from);
    for path in [&model, &crate_dir.join("shim")] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for header in ["device.h", "fxfs.h"] {
        println!(
            "cargo:rerun-if-changed={}",
            root.join("guest/linux/fs/fxfs").join(header).display()
        );
    }
    cc::Build::new()
        .file(&model)
        .file(crate_dir.join("shim/shim.c"))
        .include(crate_dir.join("shim"))
        // For a copy's `#include "../../linux/fs/fxfs/fxfs.h"`, which its own directory hasn't.
        .include(root.join("guest/sim/fxfs"))
        .std("gnu11")
        // A panic unwinds through the model to the case that ran it (shim/shim.c).
        .flag("-fexceptions")
        // The warnings guest/sim's Makefile has, not cc's -Wextra, and not errors: a mutant may
        // leave a variable unused.
        .extra_warnings(false)
        .flag("-Wall")
        .flag("-Wno-unused-function")
        .flag("-Wno-unused-variable")
        .compile("fxfs_sim_model");
}
