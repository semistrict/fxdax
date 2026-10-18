// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generates the extent daemon's gRPC API (`proto/fxdax/extentd/v1/extentd.proto`), with protox,
//! which compiles protobuf in Rust: no protoc needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "proto/fxdax/extentd/v1/extentd.proto";
    println!("cargo:rerun-if-changed={proto}");
    let descriptors = protox::compile([proto], ["proto"])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    Ok(())
}
