// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the store's tests and its benchmark (`examples/store-bench.rs`) share of the data they
//! write.

/// Bytes from a xorshift generator, which do not compress: framed, they take as much of the host's
/// disk as they are. Each seed's its own: the generator's state is the seed's bits over a
/// constant's, which no seed but the constant makes zero, and no two seeds make one, as `seed | 1`
/// did 2 and 3, whose bytes compression made one of.
pub fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}
