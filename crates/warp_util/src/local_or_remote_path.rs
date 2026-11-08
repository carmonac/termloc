use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::standardized_path::StandardizedPath;

/// Uniquely identifies where a file lives. Used across both the buffer model and the
/// editor/view layers as the canonical file-identity type.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LocalOrRemotePath {
    /// File on the local filesystem.
    Local(PathBuf),
}

impl LocalOrRemotePath {
    /// Returns the standardized path component of the location.
    pub fn path_component(&self) -> StandardizedPath {
        let LocalOrRemotePath::Local(path) = self;
        StandardizedPath::from_local_absolute_unchecked(path)
    }

    /// Returns the file name component for display (e.g. tab titles).
    pub fn display_name(&self) -> &str {
        self.file_name().unwrap_or_default()
    }

    /// Returns a displayable path string.
    pub fn display_path(&self) -> String {
        let LocalOrRemotePath::Local(path) = self;
        path.to_string_lossy().to_string()
    }

    /// Returns this location's parent.
    pub fn parent(&self) -> Option<LocalOrRemotePath> {
        let LocalOrRemotePath::Local(path) = self;
        path.parent()
            .map(|parent| LocalOrRemotePath::Local(parent.to_path_buf()))
    }

    /// Returns the file name component.
    pub fn file_name(&self) -> Option<&str> {
        let LocalOrRemotePath::Local(path) = self;
        path.file_name().and_then(|name| name.to_str())
    }

    /// Returns whether this location starts with `base`.
    pub fn starts_with(&self, base: &LocalOrRemotePath) -> bool {
        let (LocalOrRemotePath::Local(path), LocalOrRemotePath::Local(base)) = (self, base);
        path.starts_with(base)
    }

    /// Returns the local filesystem path.
    pub fn to_local_path(&self) -> Option<&Path> {
        let LocalOrRemotePath::Local(path) = self;
        Some(path.as_path())
    }

    /// Joins a (typically repo-relative) segment onto this location.
    ///
    /// If `segment` is itself absolute, the standard `Path::join` replacement semantics apply.
    pub fn join(&self, segment: &str) -> LocalOrRemotePath {
        let LocalOrRemotePath::Local(path) = self;
        LocalOrRemotePath::Local(path.join(segment))
    }

    /// If `file` starts with this location's path, returns the relative remainder as a
    /// `String`. Returns `None` when `file` is not under this location.
    pub fn strip_repo_prefix(&self, file: &LocalOrRemotePath) -> Option<String> {
        let (LocalOrRemotePath::Local(repo), LocalOrRemotePath::Local(file)) = (self, file);
        file.strip_prefix(repo)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }
}

impl From<PathBuf> for LocalOrRemotePath {
    fn from(path: PathBuf) -> Self {
        LocalOrRemotePath::Local(path)
    }
}

impl From<LocalOrRemotePath> for PathBuf {
    fn from(location: LocalOrRemotePath) -> Self {
        let LocalOrRemotePath::Local(path) = location;
        path
    }
}

impl From<&LocalOrRemotePath> for PathBuf {
    fn from(location: &LocalOrRemotePath) -> Self {
        let LocalOrRemotePath::Local(path) = location;
        path.clone()
    }
}

#[cfg(test)]
#[path = "local_or_remote_path_tests.rs"]
mod tests;
