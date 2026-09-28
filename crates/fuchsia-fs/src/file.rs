// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/lib/fuchsia-fs/src/file.rs`: reading a whole file and writing a whole buffer
// to one, with the errors those can have without opening, linking or renaming files.

use fidl_fuchsia_io as fio;
use thiserror::Error;

/// An error encountered while reading a file
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ReadError {
    #[error("read call failed: {0:?}")]
    Fidl(#[from] fidl::Error),

    #[error("read failed with status: {0}")]
    ReadError(#[source] zx_status::Status),

    #[error("file was not a utf-8 encoded string: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

/// An error encountered while writing a file
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum WriteError {
    #[error("write call failed: {0}")]
    Fidl(#[from] fidl::Error),

    #[error("write failed with status: {0}")]
    WriteError(#[source] zx_status::Status),

    #[error("file endpoint reported more bytes written than were provided")]
    Overwrite,
}

/// Writes the given data into the given file.
pub async fn write<D>(file: &fio::FileProxy, data: D) -> Result<(), WriteError>
where
    D: AsRef<[u8]>,
{
    let mut data = data.as_ref();

    while !data.is_empty() {
        let bytes_written = file
            .write(&data[..std::cmp::min(fio::MAX_BUF as usize, data.len())])
            .await?
            .map_err(|s| WriteError::WriteError(zx_status::Status::err_from_raw(s)))?;

        if bytes_written > data.len() as u64 {
            return Err(WriteError::Overwrite);
        }

        data = &data[bytes_written as usize..];
    }
    Ok(())
}

/// Reads all data from the given file's current offset to the end of the file.
pub async fn read(file: &fio::FileProxy) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();

    loop {
        let mut bytes = file
            .read(fio::MAX_BUF)
            .await?
            .map_err(|s| ReadError::ReadError(zx_status::Status::err_from_raw(s)))?;
        if bytes.is_empty() {
            break;
        }
        out.append(&mut bytes);
    }
    Ok(out)
}
