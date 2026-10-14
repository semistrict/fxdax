//! Copying a store in a directory of the host's, as fxdax's images are cloned.

use crate::layout::{DESCRIPTION, Description, EXTENTS, INDEX, SEGMENTS};
use anyhow::{Context as _, Error};
use std::fs::File;
use std::io::{self, Write as _};
use std::path::Path;

/// Copies the store in the directory `src` to a new directory `dst`, whose store's branch is
/// `branch`, copying each file with `copy`, which may share the file's blocks rather than copy
/// them. The store must not change meanwhile: no commit, write or free.
///
/// The copy holds the store as of its last commit, as after a crash. Its description is written
/// last, so until the copy is complete, `dst` is no store. A failure leaves `dst` as it is.
pub fn copy_store(
    src: &Path,
    dst: &Path,
    branch: u64,
    mut copy: impl FnMut(&File, &Path) -> io::Result<()>,
) -> Result<(), Error> {
    let description = Description::parse(
        &std::fs::read_to_string(src.join(DESCRIPTION))
            .context("reading the store's description")?,
    )?;
    std::fs::create_dir(dst).with_context(|| format!("creating {}", dst.display()))?;
    for dir in [SEGMENTS, EXTENTS, INDEX] {
        let (from, to) = (src.join(dir), dst.join(dir));
        std::fs::create_dir(&to)?;
        for entry in std::fs::read_dir(&from)? {
            let entry = entry?;
            let file = File::open(entry.path())?;
            copy(&file, &to.join(entry.file_name()))
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
        File::open(&to)?.sync_all()?;
    }
    let mut file = File::create_new(dst.join(DESCRIPTION))?;
    file.write_all(Description { branch, ..description }.text().as_bytes())?;
    file.sync_all()?;
    File::open(dst)?.sync_all()?;
    // The copy's directory entry is as durable as the copy.
    let parent = dst.parent().filter(|parent| !parent.as_os_str().is_empty());
    File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}
