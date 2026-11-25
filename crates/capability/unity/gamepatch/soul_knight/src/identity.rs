//! INPUT:  Host Config paths and the original 8.7.0 package files
//! OUTPUT: verified base/split APK, native directory and original SO identities
//! POS:    Soul Knight Title-only build gate; mismatch has no runtime side effect

#![cfg_attr(not(test), forbid(unsafe_code))]
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::io::Read;

use rudroid_common::host_api::HostApiV0;

use crate::sha256;

pub const PACKAGE: &str = "com.ChillyRoom.DungeonShooter";
pub const VERSION_NAME: &str = "8.7.0";
pub const VERSION_CODE: i64 = 80700;
pub const MIN_SDK: i64 = 23;
pub const TARGET_SDK: i64 = 36;

pub const BASE_APK: &str = "base.apk";
pub const CONFIG_SPLIT_APK: &str = "config.arm64_v8a.apk";
pub const PACK_DATA_SPLIT_APK: &str = "UnityDataAssetPack.apk";

pub const ANOGS: &str = "lib/arm64-v8a/libanogs.so";
pub const ANORT: &str = "lib/arm64-v8a/libanort.so";
#[cfg(test)]
pub const ORIGINAL_LIBRARIES: [&str; 2] = [ANOGS, ANORT];
pub const U8T: &str = "lib/arm64-v8a/libu8t.so";

/// Exact startup order recovered from the original Application. `libu8t`
/// depends on `libanort`'s versioned exports, so it is mapped only after both
/// SDK libraries completed `JNI_OnLoad` and the original `initialize` returned
/// zero. `libil2cpp` is the loader's next normal module after this callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapStep {
    Load(&'static str),
    InitializeAnoApplication,
}

pub const BOOTSTRAP_STEPS: [BootstrapStep; 4] = [
    BootstrapStep::Load(ANOGS),
    BootstrapStep::Load(ANORT),
    BootstrapStep::InitializeAnoApplication,
    BootstrapStep::Load(U8T),
];

const BASE_APK_SHA256: &str = "fe228467f98b57eb49d9ffe4becf9e7cec1941d844ace7ef4af7cc6a6f553530";
const CONFIG_SPLIT_SHA256: &str = "b670e1309bc4e79afa4306ec348f5ca1e076f434e85862198542d3c961d734ce";
const PACK_DATA_SPLIT_SHA256: &str = "1258efa93556e8abbf793cece4c2934dc923eabfc636dff52a897a41020661b6";
const ANOGS_SHA256: &str = "8f5712bc43539cf81e492af2d37424906292930adb4920f10ecf77691b1090fb";
const ANORT_SHA256: &str = "3ee88069639f8e887493a20b89b623b89888c38bd83e7fcdf56ec69569c67287";
const U8T_SHA256: &str = "7035c49c0f56f578025571ac209b0b32fbddea39854d9bf286c75c062c1a6bf8";

// Default APK classpath only. assets/audience_network.dex is a dynamically
// loaded SDK asset, not an already visible base-application DEX.
const CLASSPATH: [(&str, u64, &str); 7] = [
    ("classes.dex",9109248,"fc1911ad3994c830b53e03afca30104426dc78e5b8900d652a790c1b6b0db955"),
    ("classes2.dex",8231272,"6319de882fe3f93e479c5cc18fbaf021754e9c19dd61097798ea73fb82a4f73c"),
    ("classes3.dex",7524596,"8735e60945bacc3254b706a075fd51968a806e56f594934032d85158e2379f6f"),
    ("classes4.dex",9816912,"3792b4d60baaf23fbb76e21757a5fac8574ad31bbe27cb341c658fb8469a3a1d"),
    ("classes5.dex",9834128,"f2ce92c0d63d57139c8999c6df842918f52008440515f5da728de45f23a39595"),
    ("classes6.dex",1022796,"964fde141d0df1c5e312e54a24d3aaeb99b78e90a31287d31f9c90aed778fc92"),
    ("classes7.dex",720,"67be8178060d5f2a4cdad55721a7216e977c04ee3a48b4d5cbdef576fb5740f9"),
];

pub struct VerifiedBuild {
    pub host: HostApiV0,
    pub base_apk: PathBuf,
    pub split_apks: [PathBuf; 2],
    pub native_library_dir: PathBuf,
    pub files_dir: PathBuf,
    pub class_names: Vec<String>,
}

/// Package-only selection. A different title returns `Other` before any
/// version/build read, so this adapter is inert for every other Unity game.
pub fn selection(package: Option<&str>, version_name: Option<&str>, version_code: i64) -> Selection {
    if package != Some(PACKAGE) {
        return Selection::Other;
    }
    if version_name != Some(VERSION_NAME) || version_code != VERSION_CODE {
        return Selection::Reject("package version is not 8.7.0/80700");
    }
    Selection::Apply
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Other,
    Reject(&'static str),
    Apply,
}

pub fn verify(host: &HostApiV0) -> Result<VerifiedBuild, String> {
    verify_sdk_declarations(
        host.config_i64(c"package.minSdkVersion", -1),
        host.config_i64(c"package.targetSdkVersion", -1),
    )?;
    let base_apk = configured_path(host, c"paths.android_package_code", "")?;
    let base_apk = if base_apk.is_dir() { base_apk.join(BASE_APK) } else { base_apk };
    require_named_file(&base_apk, BASE_APK, "base APK")?;

    let count = host.config_len(c"paths.android_source_dirs");
    if count != 2 {
        return Err(format!(
            "expected exactly two split APKs ({CONFIG_SPLIT_APK}, {PACK_DATA_SPLIT_APK}), configured {count}"
        ));
    }
    let mut split_apks = Vec::with_capacity(2);
    for index in 0..count {
        let key = std::ffi::CString::new(format!("paths.android_source_dirs.{index}"))
            .expect("generated key has no NUL");
        split_apks.push(configured_path(host, &key, "")?);
    }
    let split_apks: [PathBuf; 2] = split_apks.try_into().expect("count checked above");
    require_named_file(&split_apks[0], CONFIG_SPLIT_APK, "configuration split APK")?;
    require_named_file(&split_apks[1], PACK_DATA_SPLIT_APK, "Unity data split APK")?;

    let game_files = configured_path(host, c"paths.game_files", ".")?;
    if !game_files.is_dir() {
        return Err(format!("paths.game_files is not a directory: {}", game_files.display()));
    }
    let anogs = game_files.join(ANOGS);
    let anort = game_files.join(ANORT);
    let u8t = game_files.join(U8T);
    require_file(&anogs, "original libanogs.so")?;
    require_file(&anort, "original libanort.so")?;
    require_file(&u8t, "original libu8t.so")?;

    let native_library_dir = configured_path(host, c"paths.library", "lib/arm64-v8a")?;
    let files_dir = configured_path(host, c"paths.android_files", ".")?;
    if !files_dir.is_dir() {
        return Err(format!("getFilesDir is not a directory: {}", files_dir.display()));
    }

    verify_hash(&base_apk, BASE_APK_SHA256, "base APK")?;
    verify_hash(&split_apks[0], CONFIG_SPLIT_SHA256, CONFIG_SPLIT_APK)?;
    verify_hash(&split_apks[1], PACK_DATA_SPLIT_SHA256, PACK_DATA_SPLIT_APK)?;
    verify_hash(&anogs, ANOGS_SHA256, "original libanogs.so")?;
    verify_hash(&anort, ANORT_SHA256, "original libanort.so")?;
    verify_hash(&u8t, U8T_SHA256, "original libu8t.so")?;
    verify_native_library_dir(&native_library_dir, &[&anogs, &anort, &u8t])?;
    let class_names = verified_class_names(&game_files)?;

    Ok(VerifiedBuild {
        host: *host,
        base_apk,
        split_apks,
        native_library_dir,
        files_dir,
        class_names,
    })
}

fn verified_class_names(root: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for (name, size, expected) in CLASSPATH {
        let path = root.join(name);
        let file = std::fs::File::open(&path).map_err(|e|format!("cannot open default DEX {name}: {e}"))?;
        let meta = file.metadata().map_err(|e|format!("cannot stat default DEX {name}: {e}"))?;
        if !meta.is_file() || meta.len() != size {
            return Err(format!("default DEX {name} size/type does not match the original APK"));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(size+1).read_to_end(&mut bytes).map_err(|e|format!("cannot read default DEX {name}: {e}"))?;
        let mut hash = sha256::Sha256::new();hash.update(&bytes);
        if bytes.len() as u64 != size || sha256::hex(&hash.finish()) != expected {
            return Err(format!("default DEX {name} does not match the original APK"));
        }
        // Hash and parse the same bounded buffer, never a second file read.
        names.extend(rudroid_common::dex_classes::defined_classes(&bytes)
            .map_err(|reason|format!("cannot index original default DEX {name}: {reason}"))?);
    }
    names.sort_unstable();names.dedup();
    Ok(names)
}

/// Manifest facts for the already selected exact APK, not the host's API level.
/// Reject an old/mismatched config before any protected library is initialized.
pub(super) fn verify_sdk_declarations(min: i64, target: i64) -> Result<(), String> {
    if min==MIN_SDK && target==TARGET_SDK { Ok(()) }
    else { Err("8.7.0 APK declares minSdkVersion23/targetSdkVersion36; package config must match".to_owned()) }
}

/// The path passed to the original `initialize` must be the canonical parent
/// of the exact SDK libraries whose bytes were just verified. Merely pointing
/// at some existing directory is not evidence of the package's native root.
fn verify_native_library_dir(configured: &Path, libraries: &[&Path]) -> Result<(), String> {
    let configured = configured.canonicalize().map_err(|error| {
        format!(
            "nativeLibraryDir cannot be canonicalized at {}: {error}",
            configured.display()
        )
    })?;
    if !configured.is_dir() {
        return Err(format!(
            "nativeLibraryDir is not a directory: {}",
            configured.display()
        ));
    }
    for library in libraries {
        let parent = library
            .parent()
            .ok_or_else(|| format!("verified SDK library has no parent: {}", library.display()))?
            .canonicalize()
            .map_err(|error| {
                format!(
                    "verified SDK library parent cannot be canonicalized at {}: {error}",
                    library.display()
                )
            })?;
        if configured != parent {
            return Err(format!(
                "nativeLibraryDir {} does not match the canonical parent {} of verified SDK library {}",
                configured.display(),
                parent.display(),
                library.display()
            ));
        }
    }
    Ok(())
}

fn configured_path(host: &HostApiV0, key: &CStr, fallback: &str) -> Result<PathBuf, String> {
    let value = host
        .config_string(key, 4096)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned());
    if value.is_empty() {
        return Err(format!("missing path config {}", key.to_string_lossy()));
    }
    Ok(absolute(Path::new(&value)))
}

fn require_file(path: &Path, label: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{label} is not a real file: {}", path.display()))
    }
}

fn require_named_file(path: &Path, expected_name: &str, label: &str) -> Result<(), String> {
    require_file(path, label)?;
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name) {
        return Err(format!(
            "{label} must be named {expected_name}: {}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_hash(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let found = sha256::file(path)
        .map(|digest| sha256::hex(&digest))
        .map_err(|error| format!("cannot hash {label} {}: {error}", path.display()))?;
    if found != expected {
        return Err(format!(
            "{label} does not match the original 8.7.0 build: expected {expected}, found {found}"
        ));
    }
    Ok(())
}

pub fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
        }
    })
}

#[cfg(test)]
#[path = "../tests/unit/identity.rs"]
mod tests;
