//! The part of `diagnostics_log` the `fuchsia` crate uses on hosts: installing
//! a `log` backend. Records are written to stderr.

use fidl_fuchsia_diagnostics_types::Severity;
use std::io::Write as _;

/// How to publish logs.
#[derive(Clone, Default)]
pub struct PublishOptions<'a> {
    tags: &'a [&'a str],
    minimum_severity: Option<Severity>,
    panic_prefix: &'static str,
}

impl<'a> PublishOptions<'a> {
    pub fn tags(mut self, tags: &'a [&'a str]) -> Self {
        self.tags = tags;
        self
    }

    pub fn minimum_severity(mut self, severity: impl Into<Severity>) -> Self {
        self.minimum_severity = Some(severity.into());
        self
    }

    pub fn panic_prefix(mut self, prefix: &'static str) -> Self {
        self.panic_prefix = prefix;
        self
    }
}

/// Why logging could not be set up.
#[derive(Debug)]
pub struct PublishError;

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to publish logs")
    }
}

impl std::error::Error for PublishError {}

fn level_filter(severity: Option<Severity>) -> log::LevelFilter {
    match severity {
        Some(Severity::Trace) => log::LevelFilter::Trace,
        Some(Severity::Debug) => log::LevelFilter::Debug,
        None | Some(Severity::Info) => log::LevelFilter::Info,
        Some(Severity::Warn) => log::LevelFilter::Warn,
        Some(Severity::Error) | Some(Severity::Fatal) => log::LevelFilter::Error,
        Some(Severity::__SourceBreaking { .. }) => log::LevelFilter::Info,
    }
}

struct StderrLogger {
    tags: String,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(std::io::stderr(), "[{}] {}: {}", self.tags, record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Install the stderr logger. Later calls only adjust the level, since a
/// process has one `log` backend.
pub fn initialize(options: PublishOptions<'_>) -> Result<(), PublishError> {
    log::set_max_level(level_filter(options.minimum_severity));
    // Ignore "already set": several tests may initialize logging in one process.
    let _ = log::set_boxed_logger(Box::new(StderrLogger { tags: options.tags.join(",") }));
    Ok(())
}

/// Like [`initialize`]; there is nothing asynchronous to wait for here.
pub fn initialize_sync(options: PublishOptions<'_>) {
    let _ = initialize(options);
}
