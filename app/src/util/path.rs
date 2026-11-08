use warp_util::local_or_remote_path::LocalOrRemotePath;
pub use warp_util::path::*;

/// Label used for the host of a `RemotePath`.
const UNKNOWN_HOST_LABEL: &str = "Remote host";

/// Returns the display name of a local or remote path, prefixed with the
/// host label for remote paths.
pub fn display_name_with_host(path: &LocalOrRemotePath) -> String {
    let name = path.display_name();
    match path {
        LocalOrRemotePath::Local(_) => name.to_string(),
        LocalOrRemotePath::Remote(_) => {
            format!("{UNKNOWN_HOST_LABEL}:{name}")
        }
    }
}

/// Returns the display path of a local or remote path,
/// prefixed with the host label for remote paths.
///
/// When `abbreviate_home` is true, local paths under the user's home directory
/// are abbreviated with a `~/` prefix. The flag is ignored for remote paths,
/// whose home directory lives on a different machine.
pub fn display_path_with_host(path: &LocalOrRemotePath, abbreviate_home: bool) -> String {
    match path {
        LocalOrRemotePath::Local(local_path) => {
            if abbreviate_home {
                dirs::home_dir()
                    .and_then(|home| local_path.strip_prefix(&home).ok())
                    .map(|relative| format!("~/{}", relative.display()))
                    .unwrap_or_else(|| local_path.display().to_string())
            } else {
                path.display_path()
            }
        }
        LocalOrRemotePath::Remote(_) => {
            format!("{UNKNOWN_HOST_LABEL}:{}", path.display_path())
        }
    }
}
