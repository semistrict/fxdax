// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Code shared between several modules of the vfs library.
//!
//! A stand-in for `src/storage/lib/vfs/rust/src/common.rs`, with only `IntoAny`, the
//! `attributes!` macro and `CreationMode`.

use std::sync::Arc;

/// Helper trait to allow casting into `Any` without requiring the underlying object implement the
/// trait without preventing it from being object safe (thus disallowing dynamic dispatch).
/// Since we provide a generic implementation, the size of each concrete type is known.
pub trait IntoAny: std::any::Any {
    /// Cast the given object into a `dyn std::any::Any`.
    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync + 'static>;
}

impl<T: 'static + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync + 'static> {
        self as Arc<dyn std::any::Any + Send + Sync + 'static>
    }
}

#[doc(hidden)]
pub mod __private {
    pub use paste::paste;
}

/// Helper for building [`fio::NodeAttributes2`]` given `requested` attributes. Code will only run
/// for `requested` attributes.
///
/// Example:
///
///   attributes!(
///       requested,
///       Mutable { creation_time: 123, modification_time: 456 },
///       Immutable { content_size: 789 }
///   );
///
#[macro_export]
macro_rules! attributes {
    (
        $requested:expr,
        Mutable {$($mut_a:ident: $mut_v:expr),* $(,)?},
        Immutable {$($immut_a:ident: $immut_v:expr),* $(,)?}
    ) => {
        fio::NodeAttributes2 {
            mutable_attributes: fio::MutableNodeAttributes {
                $($mut_a: if $requested.contains($crate::__attribute_query!($mut_a)) {
                    Option::from($mut_v)
                } else {
                    None
                },)*
                ..Default::default()
            },
            immutable_attributes: fio::ImmutableNodeAttributes {
                $($immut_a: if $requested.contains($crate::__attribute_query!($immut_a)) {
                    Option::from($immut_v)
                } else {
                    None
                },)*
                ..Default::default()
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __attribute_query {
    ($attr:ident) => {
        $crate::common::__private::paste! { fio::NodeAttributesQuery::[< $attr:upper >] }
    };
}

/// Represents if and how objects should be created with an open request.
#[derive(Debug, PartialEq, Eq)]
pub enum CreationMode {
    // Never create object.
    Never,
    // Object will be created if it does not exist.
    AllowExisting,
    // Create the object, will fail if it does exist.
    Always,
    // Create the object as an unnamed and temporary object.
    UnnamedTemporary,
    // Create the object as an unnamed, temporary, and unlinkable object.
    UnlinkableUnnamedTemporary,
}
