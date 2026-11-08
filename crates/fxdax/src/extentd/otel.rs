// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the daemon's spans, exported over OTLP (TASK-258, `docs/tracing.md`): of each request it
//! serves, and of what serving it costs its store, its commits and their phases, its opens and
//! forks, its page-ins, its uploads and their bucket requests, and its relief of memory
//! (`extent_remote::trace`, which makes them).
//!
//! Off unless `FXDAX_EXTENTD_TRACES` is `otlp` ([`from_env`]); then, configured by
//! OpenTelemetry's own variables: `OTEL_EXPORTER_OTLP_ENDPOINT` (or
//! `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) the collector they go to, over HTTP, protobuf
//! (`http://localhost:4318` by default), `OTEL_SERVICE_NAME` their service (`fxdax-extentd` by
//! default), and `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG` which are kept. A request, a
//! commit or an upload starts a trace, and the daemon makes thousands a second: unless
//! `OTEL_TRACES_SAMPLER` says otherwise, it keeps one in [`DEFAULT_RATIO`] of the traces they
//! start, and their spans with them (`parentbased_traceidratio`), `OTEL_TRACES_SAMPLER_ARG`, if
//! given, the ratio.

use super::wire::Header;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::sync::OnceLock;

/// The variable that turns the daemon's spans on: `otlp`, or nothing.
pub const OPTION: &str = "FXDAX_EXTENTD_TRACES";

/// The ratio of the traces the daemon keeps, unless `OTEL_TRACES_SAMPLER` or
/// `OTEL_TRACES_SAMPLER_ARG` says otherwise: one in a hundred.
pub const DEFAULT_RATIO: f64 = 0.01;

/// The service the daemon's spans are of, unless `OTEL_SERVICE_NAME` says otherwise.
pub const SERVICE: &str = "fxdax-extentd";

/// The attribute of a request's kind, as metrics name it (`metrics::KINDS`).
pub const REQUEST_KIND: &str = "fxdax.request.kind";

/// The attribute of a request's ID ([`request_id`]).
pub const REQUEST_ID: &str = "fxdax.request_id";

/// The attribute of the errno a request failed with.
pub const ERRNO: &str = "fxdax.errno";

/// The tracer provider spans go to, once started ([`install`]): what [`shutdown`] flushes.
static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Starts the daemon's spans if `FXDAX_EXTENTD_TRACES` says to: `otlp`, exported to the collector
/// OpenTelemetry's variables name. Returns whether it did; an error for another value of it, or
/// for OpenTelemetry's variables it can't take.
pub fn from_env() -> Result<bool, String> {
    let option = std::env::var(OPTION).unwrap_or_default();
    match option.as_str() {
        "" => Ok(false),
        "otlp" => {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .build()
                .map_err(|err| format!("making the OTLP exporter: {err}"))?;
            let mut provider = SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource());
            if let Some(sampler) = default_sampler()? {
                provider = provider.with_sampler(sampler);
            }
            install(provider.build());
            Ok(true)
        }
        other => Err(format!("{OPTION} must be `otlp` or unset, not `{other}`")),
    }
}

/// Makes spans from now on, with `provider`, the global tracer provider from now on; the first
/// installed alone, as the process starts.
pub fn install(provider: SdkTracerProvider) {
    if PROVIDER.set(provider.clone()).is_err() {
        log::warn!("extentd: tracing was started already");
        return;
    }
    opentelemetry::global::set_tracer_provider(provider);
    extent_remote::trace::enable(true);
}

/// Stops the daemon's spans, and exports those it made and didn't export yet, as it exits.
pub fn shutdown() {
    let Some(provider) = PROVIDER.get() else { return };
    extent_remote::trace::enable(false);
    if let Err(err) = provider.shutdown() {
        log::warn!("extentd: exporting the last spans: {err}");
    }
}

/// The daemon's resource: its service, [`SERVICE`] unless `OTEL_SERVICE_NAME` names another, as
/// the resource's builder reads it then.
fn resource() -> Resource {
    let resource = Resource::builder();
    match std::env::var_os("OTEL_SERVICE_NAME").is_some_and(|name| !name.is_empty()) {
        true => resource.build(),
        false => resource.with_service_name(SERVICE).build(),
    }
}

/// The sampler of the daemon's traces, unless `OTEL_TRACES_SAMPLER` names one, which the tracer
/// provider's builder reads then: of the traces requests, commits and uploads start, one in
/// `OTEL_TRACES_SAMPLER_ARG` (or [`DEFAULT_RATIO`]) kept, and the spans of each with it.
fn default_sampler() -> Result<Option<Sampler>, String> {
    if std::env::var_os("OTEL_TRACES_SAMPLER").is_some_and(|sampler| !sampler.is_empty()) {
        return Ok(None);
    }
    let ratio = match std::env::var("OTEL_TRACES_SAMPLER_ARG") {
        Ok(ratio) if !ratio.is_empty() => {
            ratio.parse::<f64>().ok().filter(|ratio| (0.0..=1.0).contains(ratio)).ok_or_else(
                || format!("OTEL_TRACES_SAMPLER_ARG must be a ratio, 0 to 1: {ratio}"),
            )?
        }
        _ => DEFAULT_RATIO,
    };
    Ok(Some(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio)))))
}

/// The span of the request `header` heads, a trace's root, served by a connection to the store
/// of `branch`, if it has one open, a call for the guest request `guest`, if any: `extentd.KIND`
/// (`metrics::span_name`); nothing while spans are off, at the cost of a load.
pub(super) fn request_span(
    header: &Header,
    branch: Option<u64>,
    guest: Option<&crate::request::RequestId>,
) -> extent_remote::trace::Span {
    use extent_remote::trace::{self, Span};
    if !trace::enabled() {
        return Span::none();
    }
    let span = Span::start(super::metrics::span_name(header.kind));
    match super::metrics::kind_name(header.kind) {
        Some(kind) => span.set(REQUEST_KIND, kind),
        // One metrics keep none of, as BUILD: its number.
        None => span.set_with(REQUEST_KIND, || header.kind.to_string()),
    }
    if let Some(id) = request_id(guest) {
        span.set_with(REQUEST_ID, || id.to_string());
    }
    if let Some(branch) = branch {
        span.set_branch(branch);
    }
    span
}

/// The ID of the guest request `guest` a request is a call for, if any, which its span carries as
/// [`REQUEST_ID`]: the guest's number of it, as the daemon's log records name it (`request_id`,
/// TASK-260), the one function that gives a request's ID. What a request's serving does on other
/// threads, as object storage's reads and the pager's, reads it from `crate::request::current`.
pub(super) fn request_id(guest: Option<&crate::request::RequestId>) -> Option<u64> {
    guest.map(|guest| guest.guest.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets the environment's `variable` to `value`, or removes it.
    fn set(variable: &str, value: Option<&str>) {
        // SAFETY: nextest runs each test in a process of its own, whose other threads don't read
        // the environment.
        unsafe {
            match value {
                Some(value) => std::env::set_var(variable, value),
                None => std::env::remove_var(variable),
            }
        }
    }

    #[test]
    fn tracing_is_off_unless_the_option_says_otlp() {
        set(OPTION, None);
        assert_eq!(from_env(), Ok(false));
        set(OPTION, Some("jaeger"));
        assert_eq!(
            from_env(),
            Err("FXDAX_EXTENTD_TRACES must be `otlp` or unset, not `jaeger`".to_owned())
        );
        assert!(!extent_remote::trace::enabled());
    }

    #[test]
    fn otlp_starts_tracing() {
        set(OPTION, Some("otlp"));
        set("OTEL_EXPORTER_OTLP_ENDPOINT", Some("http://localhost:4318"));
        assert_eq!(from_env(), Ok(true));
        assert!(extent_remote::trace::enabled());
        shutdown();
        assert!(!extent_remote::trace::enabled());
    }

    #[test]
    fn traces_are_sampled_by_ratio_unless_opentelemetrys_variables_say_otherwise() {
        let sampler = || format!("{:?}", default_sampler());
        set("OTEL_TRACES_SAMPLER", None);
        set("OTEL_TRACES_SAMPLER_ARG", None);
        assert_eq!(sampler(), "Ok(Some(ParentBased(TraceIdRatioBased(0.01))))");
        set("OTEL_TRACES_SAMPLER_ARG", Some("0.5"));
        assert_eq!(sampler(), "Ok(Some(ParentBased(TraceIdRatioBased(0.5))))");
        set("OTEL_TRACES_SAMPLER_ARG", Some("2"));
        assert_eq!(sampler(), "Err(\"OTEL_TRACES_SAMPLER_ARG must be a ratio, 0 to 1: 2\")");
        set("OTEL_TRACES_SAMPLER", Some("always_on"));
        assert_eq!(sampler(), "Ok(None)");
    }
}
