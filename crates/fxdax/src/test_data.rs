// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What fxdax's tests share of the data they write.

/// Bytes that don't compress, from a xorshift generator seeded by `seed`, each seed's its own:
/// framed, as a store's closed segments are, they take as much of the host's disk as they are.
pub(crate) fn noise(seed: u64, len: usize) -> Vec<u8> {
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
