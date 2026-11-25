//! INPUT: selected Android package and host paths
//! OUTPUT: package archive identities and split paths
//! POS: Android framework package file resolution
#![forbid(unsafe_code)]
use super::*;

pub(super) fn absolute(path:&std::path::Path)->std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_|if path.is_absolute(){path.to_owned()}else{std::env::current_dir().unwrap_or_else(|_|"/".into()).join(path)})
}
pub(super) extern "C" fn base_package(env:JniEnv,_context:JObject)->JObject {
    singleton(env,&BASE_PACKAGE,|| {
        let root=code_path(env,std::ptr::null_mut());let text=crate::preferences::text(env,root);env.delete_local_ref(root);
        let Some(text)=text else{return std::ptr::null_mut();};
        let path=legacy_base_package_path(&text);
        crate::preferences::string(env,&path.to_string_lossy())
    })
}

pub(super) fn split_packages(env:JniEnv)->*mut c_void {
    let Some(host)=crate::HOST.get() else{return std::ptr::null_mut();};
    let count=host.config_len(c"paths.android_source_dirs").min(rudroid_common::android_assets::MAX_PACKS as u32);
    let mut paths=Vec::new();
    for index in 0..count {
        let key=format!("paths.android_source_dirs.{index}");
        let Ok(key)=CString::new(key) else{return std::ptr::null_mut();};
        let Some(value)=host.config_string(&key,4096) else{continue;};
        if let Some(path)=split_apk_path(&value){paths.push(path);}
    }
    if paths.is_empty(){return std::ptr::null_mut();}
    let class=env.find_class(c"java/lang/String");let array=env.new_object_array(paths.len() as i32,class,std::ptr::null_mut());
    for (index,path) in paths.iter().enumerate(){
        let value=crate::preferences::string(env,&path.to_string_lossy());env.set_object_array_element(array,index as i32,value);crate::preferences::delete(env,value);
    }
    crate::preferences::delete(env,class);array
}

/// Generic sourceDir compatibility: prefer a real APK beside the configured
/// tree, but retain the extracted root when an existing title has no APK.
/// Soul Knight's exact three-container requirement is enforced in its own
/// gamepatch before any SDK call.
fn legacy_base_package_path(configured:&str)->std::path::PathBuf {
    let path=absolute(std::path::Path::new(configured));
    let candidate=if path.is_dir(){path.join("base.apk")}else{path.clone()};
    candidate.is_file().then_some(candidate)
        .or_else(||path.is_file().then_some(path.clone()))
        .unwrap_or(path)
}

/// Split source identity is a real APK file. Directories, PAD asset paths and
/// absent paths are omissions, never converted into synthetic split APK names.
fn split_apk_path(configured:&str)->Option<std::path::PathBuf> {
    if configured.is_empty(){return None;}
    let path=absolute(std::path::Path::new(configured));
    (path.is_file() && path.extension().and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("apk")))
        .then_some(path)
}


#[cfg(test)]
#[path="../tests/unit/package_paths.rs"]
mod tests;
