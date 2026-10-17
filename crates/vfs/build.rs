// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Builds the vfs library as Fuchsia builds it at API level `HEAD`, without the deprecated
//! fuchsia.io methods.

fn main() {
    for level in ["27", "28", "29", "32", "HEAD"] {
        println!("cargo::rustc-cfg=fuchsia_api_level_at_least=\"{level}\"");
    }
}
