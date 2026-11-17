use crate::{ModelError as E, Result};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub(crate) fn directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(E::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn ordinary_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(E::Storage);
    }
    Ok(())
}
pub(crate) fn open_file(path: &Path, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    if create {
        options.write(true).create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if !create && !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(E::Storage);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(E::Storage);
    }
    Ok(file)
}
/// Reopen (or create) an app-owned partial download for resuming. Never follows a link.
pub(crate) fn open_part(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(E::Storage);
    }
    Ok(file)
}
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}
pub(crate) fn remove_bundle(path: &Path) -> Result<()> {
    ordinary_directory(path)?;
    // Only ordinary files are part of a bundle. Never recurse into unrecognized trees.
    let entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries
        .iter()
        .any(|e| e.file_type().map_or(true, |kind| !kind.is_file()))
    {
        return Err(E::Storage);
    }
    for entry in entries {
        fs::remove_file(entry.path())?;
    }
    fs::remove_dir(path)?;
    Ok(())
}
pub(crate) fn check_space(available: u64, bytes: u64) -> Result<()> {
    match bytes.checked_add(64 * 1024 * 1024) {
        Some(required) if available >= required => Ok(()),
        _ => Err(E::InsufficientSpace),
    }
}
