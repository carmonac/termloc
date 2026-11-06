//! Shared utilities for file-attachment handling.

/// A downloaded file attachment with its resolved path on disk.
#[derive(Clone)]
pub(crate) struct DownloadedAttachment {
    /// The UUID of the attachment.
    /// The sanitized display name.
    pub file_name: String,
    /// The full resolved path on disk where the file was downloaded.
    pub file_path: String,
}
