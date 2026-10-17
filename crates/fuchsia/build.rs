// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's build sets the API level the SDK targets; the library
// selects its diagnostics types with it.
fn main() {
    println!("cargo:rustc-cfg=fuchsia_api_level_at_least=\"27\"");
}
