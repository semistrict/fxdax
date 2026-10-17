// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The parts of `fuchsia_inspect` that Fxfs uses, as no-ops.
//!
//! On Fuchsia, Inspect publishes a tree of diagnostics through a VMO. Here
//! the tree is never published, so nodes and properties record nothing, but
//! they keep the real library's names and signatures so Fxfs is unchanged.

use futures::future::BoxFuture;

/// Accepted wherever Inspect takes a node or property name.
pub trait Name {}
impl<T: AsRef<str>> Name for T {}

/// The root of an Inspect tree.
#[derive(Default)]
pub struct Inspector {
    root: Node,
}

impl Inspector {
    pub fn root(&self) -> &Node {
        &self.root
    }
}

pub mod component {
    use super::Inspector;
    use std::sync::LazyLock;

    /// The component-wide Inspector.
    pub fn inspector() -> &'static Inspector {
        static INSPECTOR: LazyLock<Inspector> = LazyLock::new(Inspector::default);
        &INSPECTOR
    }
}

/// A node in the tree.
#[derive(Debug, Default)]
pub struct Node;

impl Node {
    pub fn clone_weak(&self) -> Node {
        Node
    }

    pub fn create_child(&self, _name: impl Name) -> Node {
        Node
    }

    pub fn record_child(&self, _name: impl Name, record: impl FnOnce(&Node)) {
        record(&Node);
    }

    pub fn record<T>(&self, _property: T) {}

    pub fn create_lazy_child<F>(&self, _name: impl Name, _callback: F) -> LazyNode
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
        LazyNode
    }

    pub fn record_lazy_child<F>(&self, _name: impl Name, _callback: F)
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
    }

    pub fn create_uint(&self, _name: impl Name, _value: u64) -> UintProperty {
        UintProperty
    }

    pub fn record_uint(&self, _name: impl Name, _value: u64) {}

    pub fn record_bool(&self, _name: impl Name, _value: bool) {}

    pub fn record_string(&self, _name: impl Name, _value: impl AsRef<str>) {}

    pub fn create_uint_exponential_histogram(
        &self,
        _name: impl Name,
        _params: ExponentialHistogramParams<u64>,
    ) -> UintExponentialHistogramProperty {
        UintExponentialHistogramProperty
    }

    pub fn create_uint_linear_histogram(
        &self,
        _name: impl Name,
        _params: LinearHistogramParams<u64>,
    ) -> UintLinearHistogramProperty {
        UintLinearHistogramProperty
    }
}

/// A node whose contents are produced on demand.
#[derive(Debug, Default)]
pub struct LazyNode;

/// A property that can be overwritten.
pub trait Property<'t> {
    type Type;

    fn set(&self, value: Self::Type);
}

/// A numeric property that can be adjusted in place.
pub trait NumericProperty<'t>: Property<'t> {
    fn add(&self, value: Self::Type) -> Option<Self::Type>;
    fn subtract(&self, value: Self::Type) -> Option<Self::Type>;
}

/// A histogram.
pub trait HistogramProperty {
    type Type;

    fn insert(&self, value: Self::Type);
    fn insert_multiple(&self, value: Self::Type, count: usize);
    fn clear(&self);
}

macro_rules! property {
    ($name:ident, $ty:ty) => {
        #[derive(Debug, Default)]
        pub struct $name;

        impl<'t> Property<'t> for $name {
            type Type = $ty;

            fn set(&self, _value: $ty) {}
        }
    };
}

macro_rules! numeric_property {
    ($name:ident, $ty:ty) => {
        property!($name, $ty);

        impl<'t> NumericProperty<'t> for $name {
            fn add(&self, _value: $ty) -> Option<$ty> {
                None
            }

            fn subtract(&self, _value: $ty) -> Option<$ty> {
                None
            }
        }
    };
}

numeric_property!(UintProperty, u64);
numeric_property!(IntProperty, i64);
property!(BoolProperty, bool);

macro_rules! histogram {
    ($name:ident, $ty:ty) => {
        #[derive(Debug, Default)]
        pub struct $name;

        impl HistogramProperty for $name {
            type Type = $ty;

            fn insert(&self, _value: $ty) {}
            fn insert_multiple(&self, _value: $ty, _count: usize) {}
            fn clear(&self) {}
        }
    };
}

histogram!(UintExponentialHistogramProperty, u64);
histogram!(UintLinearHistogramProperty, u64);

#[derive(Clone, Debug, Default)]
pub struct ExponentialHistogramParams<T> {
    pub floor: T,
    pub initial_step: T,
    pub step_multiplier: T,
    pub buckets: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LinearHistogramParams<T> {
    pub floor: T,
    pub step_size: T,
    pub buckets: usize,
}
