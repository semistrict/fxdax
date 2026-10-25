// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The daemon's log, on standard error: what is as grave as information or more, each record a
//! line, its key-values after its message (TASK-107), as `fxfs::log::error!(error:?; "...")`
//! gives the cause of what it says went wrong there alone. The daemon's tests log so too, which
//! nextest shows of a test that fails.

use std::fmt;

/// Logs to standard error from now on, unless the process has a log already.
pub fn to_stderr() -> Result<(), log::SetLoggerError> {
    log::set_logger(&Stderr).map(|()| log::set_max_level(log::LevelFilter::Info))
}

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", Line(record));
        }
    }

    fn flush(&self) {}
}

/// `record`'s line: its level, target and message, then each key-value, `key=value`.
struct Line<'a>(&'a log::Record<'a>);

impl fmt::Display for Line<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let record = self.0;
        write!(f, "{} {}: {}", record.level(), record.target(), record.args())?;
        record.key_values().visit(&mut KeyValues(f)).map_err(|_| fmt::Error)
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

#[cfg(test)]
mod tests {
    use super::Line;

    #[test]
    fn key_values_follow_the_message() {
        let range = 0..4096u64;
        let error = std::io::Error::from_raw_os_error(libc::ENOMEM);
        let pairs: [(&str, log::kv::Value<'_>); 2] = [
            ("range", log::kv::Value::from_debug(&range)),
            ("error", log::kv::Value::from_debug(&error)),
        ];
        let line = Line(
            &log::Record::builder()
                .level(log::Level::Error)
                .target("fxfs_platform::fuchsia::fxextent")
                .args(format_args!("Failed to load an extent's range"))
                .key_values(&pairs)
                .build(),
        )
        .to_string();
        assert_eq!(
            line,
            "ERROR fxfs_platform::fuchsia::fxextent: Failed to load an extent's range range=0..4096 \
             error=Os { code: 12, kind: OutOfMemory, message: \"Cannot allocate memory\" }"
        );
    }

    #[test]
    fn a_record_without_key_values_is_its_message() {
        let line = Line(
            &log::Record::builder()
                .level(log::Level::Warn)
                .target("fxdax::extentd::stores")
                .args(format_args!("extentd: keeping branch 1 in object storage"))
                .build(),
        )
        .to_string();
        assert_eq!(
            line,
            "WARN fxdax::extentd::stores: extentd: keeping branch 1 in object storage"
        );
    }
}
