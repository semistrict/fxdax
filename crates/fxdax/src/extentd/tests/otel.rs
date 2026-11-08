// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-258: the daemon's spans, exported to OpenTelemetry's in-memory exporter, as a collector
//! would get them: a request's and a commit's trees, and their attributes; and none while tracing
//! is off.

use super::{BLOCK, connect, host, pattern, read, write};
use crate::ExtentId;
use opentelemetry::Value;
use opentelemetry_sdk::trace::{InMemorySpanExporter, Sampler, SdkTracerProvider, SpanData};
use std::collections::BTreeSet;

/// A tracer provider keeping every span, at once, in `exporter`.
fn provider(exporter: &InMemorySpanExporter) -> SdkTracerProvider {
    SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .with_sampler(Sampler::AlwaysOn)
        .build()
}

/// Writes 2 blocks, reads them back and commits them durably, in a store a daemon serves, as a VM
/// process would: returns the store's branch.
fn write_read_commit() -> u64 {
    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 20);
    let branch = store.branch();
    let extent = ExtentId { branch, sequence: 1 };
    // TASK-260: the write for the guest's request 42.
    let guest =
        std::num::NonZeroU64::new(42).map(|guest| crate::request::RequestId { guest, store: None });
    crate::request::within_now(guest, || write(&store, extent, 0, &pattern(1, 2 * BLOCK))).unwrap();
    assert_eq!(read(&store, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
    store.commit(Some(b"root")).unwrap();
    drop(store);
    served.join().unwrap();
    branch
}

/// The span of `spans` named `name`, the one there is.
fn the<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    let named: Vec<&SpanData> = spans.iter().filter(|span| span.name == name).collect();
    assert_eq!(named.len(), 1, "spans named {name}: {:?}", names(spans.iter()));
    named[0]
}

/// The names of `spans`, in order.
fn names<'a>(spans: impl Iterator<Item = &'a SpanData>) -> Vec<String> {
    spans.map(|span| span.name.to_string()).collect()
}

/// The names of the children of `parent` in `spans`, each once.
fn children(spans: &[SpanData], parent: &SpanData) -> BTreeSet<String> {
    let id = parent.span_context.span_id();
    let of = spans.iter().filter(|span| span.parent_span_id == id);
    for child in of.clone() {
        assert_eq!(child.span_context.trace_id(), parent.span_context.trace_id(), "{}", child.name);
    }
    names(of).into_iter().collect()
}

/// The attribute `key` of `span`, if it has it.
fn attribute(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes.iter().find(|kv| kv.key.as_str() == key).map(|kv| kv.value.clone())
}

/// A request served is a trace's root, named by its kind, which says the store's branch and what
/// it asked for; a durable commit's span has its phases as children, sealing, writing, the flush
/// and the wait for object storage; and the upload of its segment is a span whose children are
/// the bucket's requests.
#[test]
fn spans_trace_requests_commits_and_uploads() {
    host();
    let exporter = InMemorySpanExporter::default();
    let provider = provider(&exporter);
    super::super::otel::install(provider.clone());
    let branch = write_read_commit();
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    let hex = Value::from(format!("{branch:x}"));

    let opened = the(&spans, "extentd.open");
    assert_eq!(children(&spans, opened), BTreeSet::from(["store.create".to_owned()]));
    let created = the(&spans, "store.create");
    assert_eq!(attribute(created, "fxdax.branch"), Some(hex.clone()));

    let written = the(&spans, "extentd.write");
    assert_eq!(written.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    assert_eq!(attribute(written, "fxdax.request.kind"), Some(Value::from("write")));
    assert_eq!(attribute(written, "fxdax.branch"), Some(hex.clone()));
    assert_eq!(attribute(written, "fxdax.bytes"), Some(Value::I64(2 * BLOCK as i64)));
    assert_eq!(written.status, opentelemetry::trace::Status::Unset);
    assert_eq!(attribute(written, "fxdax.request_id"), Some(Value::from("42")));
    let read = the(&spans, "extentd.read");
    assert_eq!(attribute(read, "fxdax.bytes"), Some(Value::I64(2 * BLOCK as i64)));
    assert_eq!(attribute(read, "fxdax.request_id"), None, "a call for no guest request");

    let committed = the(&spans, "extentd.commit");
    assert_eq!(attribute(committed, "fxdax.durable"), Some(Value::Bool(true)));
    assert_eq!(children(&spans, committed), BTreeSet::from(["store.commit".to_owned()]));
    let commit = the(&spans, "store.commit");
    assert_eq!(attribute(commit, "fxdax.branch"), Some(hex.clone()));
    assert_eq!(
        children(&spans, commit),
        ["commit.seal", "commit.upload_wait", "commit.write", "store.flush"]
            .map(str::to_owned)
            .into()
    );
    let sealed = the(&spans, "commit.seal");
    assert_eq!(attribute(sealed, "fxdax.extents"), Some(Value::I64(1)));

    // The commit's segment, created, the commit's small extents in it; then the branch's
    // checkpoint, which follows its forks' moves.
    let upload = the(&spans, "upload.segment");
    assert_eq!(attribute(upload, "fxdax.branch"), Some(hex.clone()));
    assert_eq!(attribute(upload, "fxdax.extents"), Some(Value::I64(0)));
    assert_eq!(children(&spans, upload), BTreeSet::from(["bucket.create".to_owned()]));
    let checkpoint = the(&spans, "upload.checkpoint");
    assert_eq!(attribute(checkpoint, "fxdax.branch"), Some(hex));
    assert_eq!(
        children(&spans, checkpoint),
        ["bucket.create", "bucket.list"].map(str::to_owned).into()
    );
}

/// With tracing off, as the daemon starts unless told otherwise, nothing is exported: not even
/// with a tracer provider set, as another library of the process may set one.
#[test]
fn nothing_is_exported_while_tracing_is_off() {
    host();
    let exporter = InMemorySpanExporter::default();
    let provider = provider(&exporter);
    opentelemetry::global::set_tracer_provider(provider.clone());
    write_read_commit();
    provider.force_flush().unwrap();
    assert!(!extent_remote::trace::enabled());
    assert_eq!(names(exporter.get_finished_spans().unwrap().iter()), Vec::<String>::new());
}
