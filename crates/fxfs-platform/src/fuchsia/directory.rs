// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Stands in for the FxDirectory of `src/storage/fxfs/platform/src/fuchsia/directory.rs`, which
// serves directories over fuchsia.io. Nodes name it for their parent links; no directory nodes are
// created here.

/// A directory node. Uninhabited: directories are not cached as nodes.
pub enum FxDirectory {}
