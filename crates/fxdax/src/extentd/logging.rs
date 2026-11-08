// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's log, on standard error: what is as grave as information or more, each record a
//! line, its key-values after its message (TASK-107), as `fxfs::log::error!(error:?; "...")`
//! gives the cause of what it says went wrong there alone. The daemon's tests log so too, which
//! nextest shows of a test that fails.
//!
//! fxdax: a record about a guest request, the work on its thread being for one
//! (`crate::request::current`), names it: `request_id`, the guest's number of it, and `store`, the
//! store whose VM process asked, which tells one VM's requests from another's (TASK-260).
//!
//! Two variables set it, read as the log is set up:
//!
//! - `FXDAX_LOG_FORMAT`: `text`, the default, or `json`: each record a JSON object, on a line of
//!   its own, of its `timestamp` (RFC 3339, UTC, to the microsecond), `level`, `target` and
//!   `message`, its request's `request_id` and `store` if it is about one, and then each of its
//!   key-values as a field of its own: a number or boolean as such, anything else as its text. A
//!   key-value named as one of those fields is `kv_` and its name (TASK-259).
//! - `FXDAX_LOG_LEVEL`: the gravest records logged, `info` by default: `off`, `error`, `warn`,
//!   `info`, `debug` or `trace`, then, after commas, `TARGET=LEVEL`s for the targets (module paths)
//!   at or under TARGET, the longest such that names one deciding: as
//!   `info,extent_remote::store=debug`, which logs each request of object storage too.

use std::fmt::{self, Write as _};
use std::io::Write as _;
use std::time::SystemTime;

/// How the log writes its records: `FXDAX_LOG_FORMAT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A line of its level, target, message and key-values, `key=value`.
    Text,
    /// A JSON object, of a field for each of those, and the time.
    Json,
}

/// What the log takes and how it writes it: the variables the module says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// How it writes its records.
    pub format: Format,
    /// The gravest records logged of any target not named in `targets`.
    pub level: log::LevelFilter,
    /// Targets' own, each of what is at or under its target.
    pub targets: Vec<(String, log::LevelFilter)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { format: Format::Text, level: log::LevelFilter::Info, targets: Vec::new() }
    }
}

impl Settings {
    /// What this process's environment says.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// What the variables `var` gives say.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut settings = Self::default();
        match var("FXDAX_LOG_FORMAT").as_deref() {
            None | Some("" | "text") => {}
            Some("json") => settings.format = Format::Json,
            Some(other) => return Err(format!("FXDAX_LOG_FORMAT is text or json, not {other:?}")),
        }
        let levels = var("FXDAX_LOG_LEVEL").unwrap_or_default();
        let level = |text: &str| {
            text.parse::<log::LevelFilter>()
                .map_err(|_| format!("FXDAX_LOG_LEVEL: {text:?} is no level (off to trace)"))
        };
        for part in levels.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            match part.split_once('=') {
                Some((target, of)) => settings.targets.push((target.to_owned(), level(of)?)),
                None => settings.level = level(part)?,
            }
        }
        Ok(settings)
    }

    /// The gravest records of `target` logged.
    fn level_of(&self, target: &str) -> log::LevelFilter {
        let under = |of: &str| {
            target.strip_prefix(of).is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        };
        (self.targets.iter().filter(|(of, _)| under(of)))
            .max_by_key(|(of, _)| of.len())
            .map_or(self.level, |&(_, level)| level)
    }

    /// The gravest records of any target logged.
    fn max_level(&self) -> log::LevelFilter {
        self.targets.iter().map(|&(_, level)| level).fold(self.level, std::cmp::max)
    }
}

/// Logs to standard error from now on, as the environment says ([`Settings::from_env`]), unless the
/// process has a log already.
pub fn to_stderr() -> anyhow::Result<()> {
    install(Settings::from_env().map_err(anyhow::Error::msg)?)?;
    Ok(())
}

/// Logs to standard error from now on, as `settings` says, unless the process has a log already.
pub fn install(settings: Settings) -> Result<(), log::SetLoggerError> {
    install_to(settings, Sink::Stderr)
}

fn install_to(settings: Settings, sink: Sink) -> Result<(), log::SetLoggerError> {
    let max = settings.max_level();
    log::set_boxed_logger(Box::new(Logger { settings, sink })).map(|()| log::set_max_level(max))
}

/// Logs into what it returns from now on, each record a line, as the defaults say, but for its
/// level, debug: what tests of what is logged read. Once a process, as a log is.
#[cfg(test)]
pub(crate) fn capture() -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let lines = std::sync::Arc::default();
    let settings = Settings { level: log::LevelFilter::Debug, ..Settings::default() };
    install_to(settings, Sink::Captured(std::sync::Arc::clone(&lines)))
        .expect("the test's process has no log yet");
    lines
}

struct Logger {
    settings: Settings,
    sink: Sink,
}

/// Where a [`Logger`] writes its lines.
enum Sink {
    Stderr,
    #[cfg(test)]
    Captured(std::sync::Arc<std::sync::Mutex<Vec<String>>>),
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.settings.level_of(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let format = self.settings.format;
        let mut line = line(format, record, crate::request::current(), SystemTime::now());
        match &self.sink {
            Sink::Stderr => {
                line.push('\n');
                // One write of the line whole, which no other thread's comes between.
                drop(std::io::stderr().lock().write_all(line.as_bytes()));
            }
            #[cfg(test)]
            Sink::Captured(lines) => lines.lock().unwrap().push(line),
        }
    }

    fn flush(&self) {}
}

/// `record`'s line, in `format`, about `request`, if any, logged at `now`.
pub fn line(
    format: Format,
    record: &log::Record<'_>,
    request: Option<crate::request::RequestId>,
    now: SystemTime,
) -> String {
    match format {
        Format::Text => Text { record, request }.to_string(),
        Format::Json => json(record, request, now),
    }
}

/// `record`'s text: its level, target and message, then each key-value, `key=value`, and its
/// request's, if any.
struct Text<'a> {
    record: &'a log::Record<'a>,
    request: Option<crate::request::RequestId>,
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let record = self.record;
        write!(f, "{} {}: {}", record.level(), record.target(), record.args())?;
        record.key_values().visit(&mut KeyValues(f)).map_err(|_| fmt::Error)?;
        if let Some(request) = &self.request {
            write!(f, " request_id={}", request.guest)?;
            if let Some(store) = &request.store {
                write!(f, " store={store}")?;
            }
        }
        Ok(())
    }
}

/// Writes the key-values it visits, each after a space.
struct KeyValues<'a, 'b>(&'a mut fmt::Formatter<'b>);

impl<'kvs> log::kv::VisitSource<'kvs> for KeyValues<'_, '_> {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'kvs>,
        value: log::kv::Value<'kvs>,
    ) -> Result<(), log::kv::Error> {
        write!(self.0, " {key}={value}").map_err(log::kv::Error::from)
    }
}

/// The fields every JSON record has, or may, which a key-value of the same name is not.
const FIELDS: [&str; 6] = ["timestamp", "level", "target", "message", "request_id", "store"];

/// `record` as a JSON object, logged at `now`, about `request`, if any.
fn json(
    record: &log::Record<'_>,
    request: Option<crate::request::RequestId>,
    now: SystemTime,
) -> String {
    let mut out = String::from("{\"timestamp\":");
    string(&mut out, &humantime::format_rfc3339_micros(now).to_string());
    out.push_str(",\"level\":");
    string(&mut out, record.level().as_str());
    out.push_str(",\"target\":");
    string(&mut out, record.target());
    out.push_str(",\"message\":");
    string(&mut out, &record.args().to_string());
    if let Some(request) = request {
        write!(out, ",\"request_id\":{}", request.guest).expect("a String takes what is written");
        if let Some(store) = &request.store {
            out.push_str(",\"store\":");
            string(&mut out, store);
        }
    }
    // A String takes every key-value, which none fails.
    drop(record.key_values().visit(&mut JsonFields(&mut out)));
    out.push('}');
    out
}

/// Appends `text` to `out`, a JSON string.
fn string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).expect("a str is a JSON string"));
}

/// Appends the key-values it visits to a JSON object, each a field.
struct JsonFields<'a>(&'a mut String);

impl<'kvs> log::kv::VisitSource<'kvs> for JsonFields<'_> {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'kvs>,
        value: log::kv::Value<'kvs>,
    ) -> Result<(), log::kv::Error> {
        let out = &mut *self.0;
        out.push(',');
        match FIELDS.contains(&key.as_str()) {
            true => string(out, &format!("kv_{key}")),
            false => string(out, key.as_str()),
        }
        out.push(':');
        if let Some(value) = value.to_bool() {
            write!(out, "{value}")
        } else if let Some(value) = value.to_u64() {
            write!(out, "{value}")
        } else if let Some(value) = value.to_i64() {
            write!(out, "{value}")
        } else if let Some(value) = value.to_f64().filter(|value| value.is_finite()) {
            write!(out, "{value}")
        } else {
            string(out, &value.to_string());
            Ok(())
        }
        .map_err(log::kv::Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::{Format, Settings, line};
    use crate::request::RequestId;
    use std::num::NonZeroU64;
    use std::time::{Duration, SystemTime};

    /// 2026-11-08T12:34:56.789012Z.
    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_micros(1_794_141_296_789_012)
    }

    fn request(guest: u64, store: Option<&str>) -> Option<RequestId> {
        Some(RequestId { guest: NonZeroU64::new(guest).unwrap(), store: store.map(Into::into) })
    }

    #[test]
    fn key_values_follow_the_message() {
        let range = 0..4096u64;
        let error = std::io::Error::from_raw_os_error(libc::ENOMEM);
        let pairs: [(&str, log::kv::Value<'_>); 2] = [
            ("range", log::kv::Value::from_debug(&range)),
            ("error", log::kv::Value::from_debug(&error)),
        ];
        let record = log::Record::builder()
            .level(log::Level::Error)
            .target("fxfs_platform::fuchsia::fxextent")
            .args(format_args!("Failed to load an extent's range"))
            .key_values(&pairs)
            .build();
        assert_eq!(
            line(Format::Text, &record, None, at()),
            "ERROR fxfs_platform::fuchsia::fxextent: Failed to load an extent's range range=0..4096 \
             error=Os { code: 12, kind: OutOfMemory, message: \"Cannot allocate memory\" }"
        );
    }

    #[test]
    fn a_record_without_key_values_is_its_message() {
        let record = log::Record::builder()
            .level(log::Level::Warn)
            .target("fxdax::extentd::stores")
            .args(format_args!("extentd: keeping branch 1 in object storage"))
            .build();
        assert_eq!(
            line(Format::Text, &record, None, at()),
            "WARN fxdax::extentd::stores: extentd: keeping branch 1 in object storage"
        );
    }

    /// A record about a guest request names it, and its store, after its key-values.
    #[test]
    fn a_text_record_names_its_request() {
        let pairs: [(&str, log::kv::Value<'_>); 1] = [("kind", log::kv::Value::from("get"))];
        let record = log::Record::builder()
            .level(log::Level::Debug)
            .target("extent_remote::store")
            .args(format_args!("object storage: get b/1"))
            .key_values(&pairs)
            .build();
        assert_eq!(
            line(Format::Text, &record, request(42, Some("/stores/vm1")), at()),
            "DEBUG extent_remote::store: object storage: get b/1 kind=get request_id=42 \
             store=/stores/vm1"
        );
    }

    /// A JSON record: its time, level, target and message, its request's, and each key-value by
    /// its type, every string escaped, a key-value named as a field renamed.
    #[test]
    fn a_json_record_has_a_field_for_each_of_its_parts() {
        let error = std::io::Error::from_raw_os_error(libc::ENOMEM);
        let pairs: [(&str, log::kv::Value<'_>); 7] = [
            ("len", log::kv::Value::from(4096u64)),
            ("delta", log::kv::Value::from(-3i64)),
            ("ms", log::kv::Value::from(1.5f64)),
            ("sealed", log::kv::Value::from(true)),
            ("path", log::kv::Value::from("a \"b\"\n\tc\u{1}")),
            ("error", log::kv::Value::from_display(&error)),
            ("level", log::kv::Value::from("mine")),
        ];
        let record = log::Record::builder()
            .level(log::Level::Warn)
            .target("fxdax::extentd::server")
            .args(format_args!("extentd: a \"quoted\" message\\ with\na line"))
            .key_values(&pairs)
            .build();
        assert_eq!(
            line(Format::Json, &record, request(7, Some("/stores/vm \"2\"")), at()),
            r#"{"timestamp":"2026-11-08T12:34:56.789012Z","level":"WARN","target":"fxdax::extentd::server","message":"extentd: a \"quoted\" message\\ with\na line","request_id":7,"store":"/stores/vm \"2\"","len":4096,"delta":-3,"ms":1.5,"sealed":true,"path":"a \"b\"\n\tc\u0001","error":"Cannot allocate memory (os error 12)","kv_level":"mine"}"#
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&line(Format::Json, &record, None, at())).unwrap();
        assert_eq!(parsed["level"], "WARN");
        assert_eq!(parsed["request_id"], serde_json::Value::Null, "no request, no field");
        assert_eq!(parsed["path"], "a \"b\"\n\tc\u{1}");
    }

    /// FXDAX_LOG_FORMAT switches the log's records from text to JSON; anything but the two is
    /// refused.
    #[test]
    fn the_format_is_the_variables() {
        let vars = |format: Option<&'static str>, level: Option<&'static str>| {
            move |name: &str| match name {
                "FXDAX_LOG_FORMAT" => format.map(str::to_owned),
                "FXDAX_LOG_LEVEL" => level.map(str::to_owned),
                _ => None,
            }
        };
        let record = log::Record::builder()
            .level(log::Level::Info)
            .target("fxdax::extentd")
            .args(format_args!("extentd: serving"))
            .build();
        let written = |format, level| {
            let settings = Settings::from_vars(vars(format, level)).unwrap();
            line(settings.format, &record, None, at())
        };
        assert_eq!(written(None, None), "INFO fxdax::extentd: extentd: serving");
        assert_eq!(written(Some("text"), None), "INFO fxdax::extentd: extentd: serving");
        assert_eq!(
            written(Some("json"), None),
            r#"{"timestamp":"2026-11-08T12:34:56.789012Z","level":"INFO","target":"fxdax::extentd","message":"extentd: serving"}"#
        );
        assert_eq!(
            Settings::from_vars(vars(Some("yaml"), None)),
            Err("FXDAX_LOG_FORMAT is text or json, not \"yaml\"".to_owned())
        );
        let levels = Settings::from_vars(vars(
            None,
            Some("warn, extent_remote=debug,extent_remote::cache=error"),
        ))
        .unwrap();
        assert_eq!(levels.level_of("fxdax::extentd::server"), log::LevelFilter::Warn);
        assert_eq!(levels.level_of("extent_remote::store"), log::LevelFilter::Debug);
        assert_eq!(levels.level_of("extent_remote::cache"), log::LevelFilter::Error);
        assert_eq!(levels.level_of("extent_remote_other"), log::LevelFilter::Warn);
        assert_eq!(levels.max_level(), log::LevelFilter::Debug);
        assert_eq!(
            Settings::from_vars(vars(None, Some("loud"))),
            Err("FXDAX_LOG_LEVEL: \"loud\" is no level (off to trace)".to_owned())
        );
    }
}
