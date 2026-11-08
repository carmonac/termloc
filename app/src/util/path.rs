use warp_util::local_or_remote_path::LocalOrRemotePath;
pub use warp_util::path::*;

/// Returns the display name of a path.
pub fn display_name_with_host(path: &LocalOrRemotePath) -> String {
    path.display_name().to_string()
}

/// Returns the display path of a path.
///
/// When `abbreviate_home` is true, paths under the user's home directory are abbreviated with a
/// `~/` prefix.
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
    }
}
