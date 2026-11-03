//! INPUT:  isolated extracted base/pack trees and typed resolver requests
//! OUTPUT:  cross-frontend path identity, precedence, failure and bounded reply regressions
//! POS:    checks real filesystem behavior without modifying any game or process cwd
use super::*;
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rudroid-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, content: &str) {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    fn resolver(&self) -> AssetResolver {
        AssetResolver::new(self.0.clone(), None, false)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn answer(
    resolver: &AssetResolver,
    kind: u32,
    path: &CStr,
    pack: Option<&CStr>,
) -> Result<Option<PathBuf>, AssetError> {
    Ok(resolver
        .query(&AssetRequest::new(kind, path, pack, 0))?
        .map(|b| CString::from_vec_with_nul(b).unwrap())
        .map(|c| PathBuf::from(OsStr::from_bytes(c.to_bytes()))))
}

#[test]
fn pack_overlay_is_presence_selected_and_critical_payload_never_falls_back() {
    let t = Tree::new();
    t.file("assets/bin/Data/data.unity3d", "base");
    t.file(
        "assetpacks/UnityDataAssetPack/assets/aa/settings.json",
        "pack",
    );
    let off = t.resolver();
    assert_eq!(
        answer(
            &off,
            PATH,
            c"jar:file:/split_UnityDataAssetPack.apk!/assets/aa/settings.json",
            None
        ),
        Err(AssetError::Missing)
    );
    let on = off.with_packs().unwrap();
    assert_eq!(
        answer(
            &on,
            PATH,
            c"jar:file:/split_UnityDataAssetPack.apk!/assets/aa/settings.json",
            None
        )
        .unwrap(),
        Some(t.0.join("assetpacks/UnityDataAssetPack/assets/aa/settings.json"))
    );
    assert_eq!(
        answer(
            &on,
            PATH,
            c"jar:file:/split_UnityDataAssetPack.apk!/assets/bin/Data/data.unity3d",
            None
        ),
        Err(AssetError::Missing)
    );
    t.file("assets/ordinary.txt", "base fallback");
    assert_eq!(
        answer(
            &on,
            PATH,
            c"split_UnityDataAssetPack.apk/assets/ordinary.txt",
            None
        )
        .unwrap(),
        Some(t.0.join("assets/ordinary.txt"))
    );
}
#[test]
fn namespace_frontends_share_base_first_merged_lookup_and_explicit_pack_identity() {
    let t = Tree::new();
    t.file("assets/a", "base");
    t.file("assetpacks/UnityDataAssetPack/assets/a", "pack");
    let r = t.resolver().with_packs().unwrap();
    for uri in [
        c"/android_asset/a",
        c"file:///android_asset/a",
        c"file://android_asset/a",
        c"jar:file://!/assets/a",
    ] {
        assert_eq!(
            answer(&r, PATH, uri, None).unwrap(),
            Some(t.0.join("assets/a"))
        );
    }
    assert_eq!(
        answer(&r, PACK, c"a", Some(c"UnityDataAssetPack")).unwrap(),
        Some(t.0.join("assetpacks/UnityDataAssetPack/assets/a"))
    );
    assert_eq!(answer(&r, PATH, c"native-file", None).unwrap(), None);
}
#[test]
fn unsafe_paths_and_unknown_queries_fail_explicitly() {
    let t = Tree::new();
    let r = t.resolver().with_packs().unwrap();
    for path in [c"../save", c"/tmp/save", c"a/../../save", c"a\\save", c""] {
        assert_eq!(answer(&r, MERGED, path, None), Err(AssetError::Invalid));
    }
    assert_eq!(
        answer(&r, PACK_ROOT, c"", Some(c"../Other")),
        Err(AssetError::Invalid)
    );
    assert_eq!(
        r.query(&AssetRequest::new(99, c"", None, 0)),
        Err(AssetError::Invalid)
    );
}
#[test]
fn split_metadata_survives_apk_removal_and_directory_names_are_sorted_and_merged() {
    let t = Tree::new();
    t.file("assets/sub/a", "base");
    t.file("assetpacks/UnityDataAssetPack/assets/sub/a", "pack");
    t.file("assetpacks/UnityDataAssetPack/assets/sub/b", "pack");
    let r = t.resolver().with_packs().unwrap();
    assert!(!t.0.join("split_UnityDataAssetPack.apk").exists());
    assert_eq!(
        answer(&r, SPLIT, c"", None).unwrap(),
        Some(t.0.join("split_UnityDataAssetPack.apk"))
    );
    assert_eq!(
        r.query(&AssetRequest::new(DIRECTORY, c"sub", None, 0))
            .unwrap(),
        Some(b"a\0b\0\0".to_vec())
    );
}
#[test]
fn font_cache_and_analytics_policies_do_not_modify_resources() {
    let t = Tree::new();
    let cache = t.0.join("cache");
    let r = AssetResolver::new(t.0.clone(), Some(cache.clone()), true);
    assert_eq!(
        answer(&r, PATH, c"/system/fonts/CJK.ttf", None).unwrap(),
        Some(t.0.join("fonts/CJK.ttf"))
    );
    assert_eq!(
        answer(&r, PATH, c"/etc/fonts.xml", None).unwrap(),
        Some(t.0.join("fonts/fonts.xml"))
    );
    assert_eq!(
        answer(&r, PATH, c"/Android/UnityShaderCache/a", None).unwrap(),
        Some(cache.join("a"))
    );
    assert_eq!(
        answer(&r, PATH, c"/files/Analytics/events/a", None).unwrap(),
        Some(PathBuf::from("/dev/null"))
    );
    assert_eq!(
        answer(&r, PATH, c"/system/fonts/../save", None),
        Err(AssetError::Invalid)
    );
    assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 0);
}
