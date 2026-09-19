// SPDX-License-Identifier: Apache-2.0
use anyhow::{ensure, Context, Result};
use oci_spec::runtime::Spec;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;

pub(crate) const MAX_BYTES: usize = 4096;

pub(crate) fn prepare(spec: &Spec) -> Result<Option<Arc<File>>> {
    let Some(annotations) = spec.annotations() else {
        return Ok(None);
    };
    if annotations
        .get("io.kubernetes.container.terminationMessagePolicy")
        .map(String::as_str)
        != Some("File")
    {
        return Ok(None);
    }
    let Some(destination) = annotations
        .get("io.kubernetes.container.terminationMessagePath")
        .filter(|p| !p.is_empty())
    else {
        return Ok(None);
    };
    let source = spec.mounts().as_ref().and_then(|mounts| {
        mounts
            .iter()
            .find(|m| m.destination() == Path::new(destination))
            .and_then(|m| m.source().as_ref())
    });
    source
        .map(|path| open_existing(path).map(Arc::new))
        .transpose()
}

fn open_existing(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "termination file path must be absolute");
    let parent = safe_path::PinnedPathBuf::from_path(path.parent().context("missing parent")?)?;
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(
            parent
                .as_path()
                .join(path.file_name().context("missing name")?),
        )?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "termination target must be a regular file with one link"
    );
    Ok(file)
}

pub(crate) fn write(file: &File, text: &str) -> Result<()> {
    let end = text.floor_char_boundary(text.len().min(MAX_BYTES));
    // Use the descriptor validated before guest execution, never reopen a guest-influenced path.
    file.set_len(0)?;
    file.write_all_at(&text.as_bytes()[..end], 0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_bytes_and_pins_original_file_without_following_replacements() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("message");
        std::fs::write(&path, "old contents").unwrap();
        let file = open_existing(&path).unwrap();
        let moved = temp.path().join("original");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        write(&file, &"é".repeat(4096)).unwrap();
        assert_eq!(std::fs::metadata(&moved).unwrap().len(), 4096);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        assert!(open_existing(&path).is_err());
        assert!(open_existing(temp.path()).is_err());
        assert!(open_existing(&temp.path().join("missing")).is_err());
    }
}
