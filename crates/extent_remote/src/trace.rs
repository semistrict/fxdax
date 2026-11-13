// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: spans of the expensive operations of extent stores, their uploads and their daemon, for
//! OpenTelemetry (TASK-258), shared by the crates that make them: extent_remote's bucket requests,
//! uploads and checkpoints, extent_store's commits and flushes, and fxdax's daemon
//! (`fxdax::extentd`), which enables them from the environment (`fxdax::extentd::otel`;
//! `docs/tracing.md`).
//!
//! Off unless enabled, a [`Span`] is nothing: made at the cost of one relaxed load, with no tracer,
//! no exporter and nothing allocated, as every request and commit makes some. On, each is a span of
//! the global tracer provider's tracer, a child of the context current where it starts: of the
//! span a thread runs in ([`Span::run`]), or a future runs around, on whatever thread polls it
//! ([`Span::around`]). It ends as it drops, its attributes and its status set on it as it goes
//! ([`Span::set`], [`Span::fail`]).

use crate::layout::Branch;
use futures::future::Either;
use opentelemetry::context::{FutureExt as _, WithContext};
use opentelemetry::trace::{Status, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, KeyValue, Value, global};
use std::future::Future;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The attribute of the bytes an operation read, wrote or moved.
pub const BYTES: &str = "fxdax.bytes";
/// The attribute of the branch an operation is of (`doc-7`), in hexadecimal as paths name it.
pub const BRANCH: &str = "fxdax.branch";
/// The attribute of an extent's sequence number in its branch ([`BRANCH`]).
pub const EXTENT: &str = "fxdax.extent";
/// The attribute of the extents an operation took.
pub const EXTENTS: &str = "fxdax.extents";
/// The attribute of the commits an operation took, or the epoch of the one it made.
pub const COMMITS: &str = "fxdax.commits";
/// The attribute of the epoch of a commit.
pub const EPOCH: &str = "fxdax.epoch";
/// The attribute of whether a commit is durable, or ordered alone.
pub const DURABLE: &str = "fxdax.durable";
/// The attribute of the branch a fork makes.
pub const CHILD: &str = "fxdax.child";
/// The attribute of whether a fork makes a point (`doc-7`, Points).
pub const POINT: &str = "fxdax.point";
/// The attribute of a store's path.
pub const STORE: &str = "fxdax.store";
/// The attribute of the arena of physical memory an operation is of.
pub const ARENA: &str = "fxdax.arena";
/// The attribute of an object's key in the bucket, without the bucket's prefix.
pub const OBJECT_KEY: &str = "fxdax.object_key";
/// The attribute of the objects a bucket's listing found, or a batch of deletes deleted.
pub const OBJECTS: &str = "fxdax.objects";
/// The attribute of the prefixes a bucket's listing by delimiter found (TASK-395.5).
pub const PREFIXES: &str = "fxdax.prefixes";
/// The attribute of a segment's sequence number in its branch.
pub const SEGMENT: &str = "fxdax.segment";

/// Whether spans are made, which [`enable`] says.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The tracer spans are made with, of the global tracer provider as [`enable`] found it.
static TRACER: OnceLock<global::BoxedTracer> = OnceLock::new();

/// The name of the tracer, the instrumentation's scope.
const SCOPE: &str = "fxdax";

/// Makes spans from now on, with the global tracer provider, which the caller set
/// (`opentelemetry::global::set_tracer_provider`) before; or, `on` false, no more.
pub fn enable(on: bool) {
    if on {
        TRACER.get_or_init(|| global::tracer(SCOPE));
    }
    ENABLED.store(on, Ordering::Release);
}

/// Whether spans are made ([`enable`]).
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The IDs of the span current on this thread, if spans are on and one is: its trace's and its
/// own, in hexadecimal, as exporters name them, which a log record says to be found in its trace.
/// Asked off the hot path: it reads the context current.
pub fn current_ids() -> Option<(String, String)> {
    if !enabled() {
        return None;
    }
    let context = Context::current();
    let span = context.span();
    let ids = span.span_context();
    ids.is_valid().then(|| (ids.trace_id().to_string(), ids.span_id().to_string()))
}

/// A span of an operation, ended as it drops; nothing while spans are off ([`enabled`]).
#[must_use = "a span ends as it drops"]
pub struct Span(Option<Context>);

impl Span {
    /// A span named `name`, a child of the context current on this thread (or of a future's
    /// context, [`Span::around`]); nothing while spans are off.
    #[inline]
    pub fn start(name: &'static str) -> Self {
        if !enabled() {
            return Self(None);
        }
        Self::start_in(name, &Context::current())
    }

    /// Nothing, as a span is while spans are off: of an operation that isn't traced.
    pub const fn none() -> Self {
        Self(None)
    }

    #[cold]
    fn start_in(name: &'static str, parent: &Context) -> Self {
        let Some(tracer) = TRACER.get() else { return Self(None) };
        let span = tracer.start_with_context(name, parent);
        Self(Some(parent.with_span(span)))
    }

    /// Whether it is a span, not nothing: what saying much of it waits for.
    #[inline]
    pub fn is_on(&self) -> bool {
        self.0.is_some()
    }

    /// Sets its attribute `key` to `value`.
    pub fn set(&self, key: &'static str, value: impl Into<Value>) {
        if let Some(context) = &self.0 {
            context.span().set_attribute(KeyValue::new(key, value));
        }
    }

    /// Sets its attribute `key` to what `value` makes, made only if it is a span: an attribute
    /// costing something to make, as a path's text.
    pub fn set_with<V: Into<Value>>(&self, key: &'static str, value: impl FnOnce() -> V) {
        if let Some(context) = &self.0 {
            context.span().set_attribute(KeyValue::new(key, value()));
        }
    }

    /// Sets its attribute `key` to `value`, a count: OpenTelemetry's integers are signed.
    pub fn set_count(&self, key: &'static str, value: u64) {
        self.set(key, i64::try_from(value).unwrap_or(i64::MAX));
    }

    /// Sets its attribute [`BRANCH`] to `branch`, in hexadecimal, as store paths name branches.
    pub fn set_branch(&self, branch: Branch) {
        self.set_with(BRANCH, || format!("{branch}"));
    }

    /// Says the operation failed, with `error`, its status an error's.
    pub fn fail(&self, error: impl std::fmt::Display) {
        if let Some(context) = &self.0 {
            context.span().set_status(Status::error(error.to_string()));
        }
    }

    /// Says the operation failed if `result` did, as [`Span::fail`] says.
    pub fn result<T, E: std::fmt::Display>(&self, result: &Result<T, E>) {
        if let Err(error) = result {
            self.fail(format_args!("{error:#}"));
        }
    }

    /// Runs `operation`, of which it is the span, its context current meanwhile ([`Span::enter`]),
    /// saying whether it failed ([`Span::result`]).
    pub fn run<T, E: std::fmt::Display>(
        &self,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let entered = self.enter();
        let result = operation();
        self.result(&result);
        drop(entered);
        result
    }

    /// Makes it the context current on this thread until the guard returned drops: the parent of
    /// the spans started meanwhile ([`Span::start`]). Nothing while it is nothing.
    pub fn enter(&self) -> Option<opentelemetry::ContextGuard> {
        self.0.as_ref().map(|context| context.clone().attach())
    }

    /// `future`, with it as the context current as each poll of it runs: the parent of the spans
    /// it starts, on whatever thread polls it. `future` itself while it is nothing.
    pub fn around<F: Future>(&self, future: F) -> Either<F, WithContext<F>> {
        self.parent().around(future)
    }

    /// It, as the parent of what another task does for it, apart from it ([`Parent::around`]).
    pub fn parent(&self) -> Parent {
        Parent(self.0.clone())
    }

    /// `future`, around which it is ([`Span::around`]), ending as `future` does: a span of an
    /// operation another task runs, as a page-in spawned.
    pub async fn instrument<F: Future>(self, future: F) -> F::Output {
        let output = self.around(future).await;
        drop(self);
        output
    }
}

/// A span, as the parent of what another task does for it ([`Span::parent`]), which ends it not.
pub struct Parent(Option<Context>);

impl Parent {
    /// `future`, with the span as the context current as each poll of it runs, as
    /// [`Span::around`] says.
    pub fn around<F: Future>(self, future: F) -> Either<F, WithContext<F>> {
        match self.0 {
            Some(context) => Either::Right(future.with_context(context)),
            None => Either::Left(future),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(context) = &self.0 {
            context.span().end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Off, a span is nothing, with no tracer made, and what is said of it is never made: it costs
    /// what a relaxed load does, as a bucket's request's span (its key, its bytes, its error) does.
    /// Prints what one costs, the fastest of 5 rounds of a million (`docs/tracing.md` has the
    /// figures), held to no bound: a debug build on a loaded CI runner took 63 ns.
    #[test]
    fn spans_off_are_nothing_and_cost_a_load() {
        let request = || {
            let span = Span::start("bucket.get");
            span.set_with(OBJECT_KEY, || -> String {
                unreachable!("an attribute made for nothing")
            });
            span.set_count(BYTES, 4096);
            span.result(&Err::<(), _>("failed"));
            std::hint::black_box(span)
        };
        assert!(request().0.is_none());
        assert!(TRACER.get().is_none());
        const SPANS: u32 = 1_000_000;
        let round = || {
            let started = Instant::now();
            for _ in 0..SPANS {
                drop(request());
            }
            started.elapsed() / SPANS
        };
        let fastest = (0..5).map(|_| round()).min().unwrap();
        println!("a span off costs {fastest:?}");
    }
}
