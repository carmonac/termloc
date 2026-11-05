use super::is_warp_bundle;

#[test]
fn is_warp_bundle_recognises_termloc() {
    assert!(is_warp_bundle("dev.termloc.Termloc"));
}

#[test]
fn is_warp_bundle_rejects_other_apps() {
    assert!(!is_warp_bundle("dev.warp.Warp"));
    assert!(!is_warp_bundle("com.microsoft.VSCode"));
    assert!(!is_warp_bundle("com.apple.TextEdit"));
    assert!(!is_warp_bundle("dev.zed.Zed"));
    assert!(!is_warp_bundle("invalid"));
    assert!(!is_warp_bundle(""));
}
