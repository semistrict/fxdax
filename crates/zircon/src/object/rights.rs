// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file at the repo's root.

//! Default rights for new objects.
//!
//! From `zircon/system/public/zircon/rights.h`.

use zx_types::*;

pub const ZX_RIGHTS_BASIC: zx_rights_t =
    ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_WAIT | ZX_RIGHT_INSPECT;
pub const ZX_RIGHTS_IO: zx_rights_t = ZX_RIGHT_READ | ZX_RIGHT_WRITE;
pub const ZX_RIGHTS_PROPERTY: zx_rights_t = ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_SET_PROPERTY;

pub const ZX_DEFAULT_EVENT_RIGHTS: zx_rights_t = ZX_RIGHTS_BASIC | ZX_RIGHT_SIGNAL;
pub const ZX_DEFAULT_PORT_RIGHTS: zx_rights_t = (ZX_RIGHTS_BASIC & !ZX_RIGHT_WAIT) | ZX_RIGHTS_IO;
pub const ZX_DEFAULT_VMO_RIGHTS: zx_rights_t =
    ZX_RIGHTS_BASIC | ZX_RIGHTS_IO | ZX_RIGHTS_PROPERTY | ZX_RIGHT_MAP | ZX_RIGHT_SIGNAL;
pub const ZX_DEFAULT_VMAR_RIGHTS: zx_rights_t =
    (ZX_RIGHTS_BASIC & !ZX_RIGHT_WAIT) | ZX_RIGHT_OP_CHILDREN;
pub const ZX_DEFAULT_PAGER_RIGHTS: zx_rights_t =
    ZX_RIGHT_INSPECT | ZX_RIGHT_TRANSFER | ZX_RIGHT_ATTACH_VMO | ZX_RIGHT_MANAGE_VMO;
