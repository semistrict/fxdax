// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: Vec<u8>| {
    fxfs::object_store::journal::super_block::fuzz::fuzz_super_block_bytes(input)
});
