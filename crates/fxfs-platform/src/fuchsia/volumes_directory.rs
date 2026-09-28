// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Stands in for the VolumesDirectory of `src/storage/fxfs/platform/src/fuchsia/volumes_directory.rs`,
// which serves a filesystem's volumes over fuchsia.fxfs and limits the dirty bytes of all of them.
// Volumes name it for their parent links; no volumes directory is created here.

/// The directory of a filesystem's volumes. Uninhabited: volumes have no parent.
pub enum VolumesDirectory {}
