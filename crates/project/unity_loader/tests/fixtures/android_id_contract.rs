//! INPUT: actual shared VM/framework/profile, fresh temporary preferences and synthetic UTF-16
//! OUTPUT: the title's read() cache/null/exception policy through production JNI services
//! POS: no APK, device identifier, fake JNI table or second capability link
#![forbid(unsafe_code)]
#[path = "../../../../capability/unity/gamepatch/soul_knight/src/android_id.rs"]
mod android_id;

use rudroid_common::jni_env::{Arg, JFieldId, JObject, JniEnv, Method};
use std::path::Path;

struct Frame(JniEnv);
impl Frame {
    fn enter(env: JniEnv) -> Self {
        assert_eq!(env.push_local_frame(64), 0);
        Self(env)
    }
}
impl Drop for Frame {
    fn drop(&mut self) { self.0.pop_local_frame(std::ptr::null_mut()); }
}

struct ActivityRestore {
    env: JniEnv,
    class: JObject,
    field: JFieldId,
    original: JObject,
}
impl Drop for ActivityRestore {
    fn drop(&mut self) {
        // The original class/field/Activity are held by the outer fixture frame.
        // Restore the public field even if an assertion unwinds with an error;
        // retain that same pending throwable instead of replacing or hiding it.
        let pending = self.env.exception_occurred();
        if !pending.is_null() { self.env.exception_clear(); }
        self.env.set_static_object_field(self.class, self.field, self.original);
        if !pending.is_null() {
            self.env.throw(pending);
            self.env.delete_local_ref(pending);
        }
    }
}

fn balanced_read(env: JniEnv) -> Result<Option<Vec<u16>>, android_id::Error> {
    assert!(!env.exception_check());
    assert_eq!(env.push_local_frame(8), 0);
    let canary = env.new_string_utf(c"android-id caller-frame canary");
    let weak = env.new_weak_global_ref(canary);
    assert!(!weak.is_null());
    let result = android_id::read(env);
    assert!(!env.exception_check());
    assert!(!env.is_same_object(weak, std::ptr::null_mut()), "read must not pop its caller's local frame");
    env.pop_local_frame(std::ptr::null_mut());
    assert!(env.is_same_object(weak, std::ptr::null_mut()), "read must not leave an extra local frame above its caller");
    env.delete_weak_global_ref(weak);
    result
}

struct Preferences {
    object: JObject,
    edit: Method,
    contains: Method,
    get: Method,
    put: Method,
    remove: Method,
    commit: Method,
    apply: Method,
}
impl Preferences {
    fn new(env: JniEnv, activity: JObject) -> Self {
        let context = env.find_class(c"android/content/Context");
        let name = env.new_string_utf(c"share_data");
        let object = env.call_object(activity, env.method(context, c"getSharedPreferences",
            c"(Ljava/lang/String;I)Landroid/content/SharedPreferences;").unwrap(), &[Arg::object(name), Arg::int(0)]);
        let prefs = env.find_class(c"android/content/SharedPreferences");
        let editor = env.find_class(c"android/content/SharedPreferences$Editor");
        assert!(!object.is_null() && !env.exception_check());
        Self {
            object,
            edit: env.method(prefs, c"edit", c"()Landroid/content/SharedPreferences$Editor;").unwrap(),
            contains: env.method(prefs, c"contains", c"(Ljava/lang/String;)Z").unwrap(),
            get: env.method(prefs, c"getString", c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;").unwrap(),
            put: env.method(editor, c"putString", c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;").unwrap(),
            remove: env.method(editor, c"remove", c"(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;").unwrap(),
            commit: env.method(editor, c"commit", c"()Z").unwrap(),
            apply: env.method(editor, c"apply", c"()V").unwrap(),
        }
    }
    fn contains(&self, env: JniEnv, key: JObject) -> bool {
        env.call_boolean(self.object, self.contains, &[Arg::object(key)])
    }
    fn get(&self, env: JniEnv, key: JObject) -> Option<Vec<u16>> {
        let value = env.call_object(self.object, self.get, &[Arg::object(key), Arg::object(std::ptr::null_mut())]);
        let text = if value.is_null() { None } else { env.string_utf16(value) };
        env.delete_local_ref(value);
        text
    }
    fn editor(&self, env: JniEnv) -> JObject { env.call_object(self.object, self.edit, &[]) }
    fn put(&self, env: JniEnv, editor: JObject, key: JObject, value: &[u16]) {
        let text = env.new_string(value);
        let returned = env.call_object(editor, self.put, &[Arg::object(key), Arg::object(text)]);
        assert!(env.is_same_object(returned, editor));
        env.delete_local_ref(returned);
        env.delete_local_ref(text);
    }
}

pub fn check(env: JniEnv, activity: JObject, root: &Path) {
    assert!(!env.exception_check());
    let _frame = Frame::enter(env);
    let path = root.join("files 数据/shared_prefs/share_data.json");
    // The runner owns a newly created profile for each child. Reject an old
    // file before opening preferences, rather than inspecting any existing ID.
    assert!(!path.exists() && !path.is_symlink(), "Android ID fixture needs a fresh synthetic store");
    let player = env.find_class(c"com/unity3d/player/UnityPlayer");
    let field = env.get_static_field_id(player, c"currentActivity", c"Landroid/app/Activity;");
    let original = env.get_static_object_field(player, field);
    assert!(env.is_same_object(original, activity));
    let restore = ActivityRestore { env, class: player, field, original };
    let preferences = Preferences::new(env, activity);
    let key = env.new_string_utf(c"android_id");
    let other = env.new_string_utf(c"android-id-fixture-other");
    assert!(!preferences.contains(env, key));

    // This is the actual framework Settings service, whose unavailable reply
    // is null. No provider override supplies a hardware identifier.
    let context = env.find_class(c"android/content/Context");
    let resolver = env.call_object(activity, env.method(context, c"getContentResolver", c"()Landroid/content/ContentResolver;").unwrap(), &[]);
    let secure = env.find_class(c"android/provider/Settings$Secure");
    let setting = env.call_static_object(secure, env.static_method(secure, c"getString",
        c"(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;").unwrap(), &[Arg::object(resolver), Arg::object(key)]);
    assert!(setting.is_null() && !env.exception_check());
    assert_eq!(balanced_read(env), Ok(None));
    assert!(!preferences.contains(env, key));
    assert!(path.is_file(), "the null-result path must perform its real commit");

    // apply changes memory only. The source's subsequent empty-editor commit
    // must persist both keys, so a missing commit cannot pass this assertion.
    let other_units: Vec<u16> = "synthetic 保留\0value".encode_utf16().collect();
    let editor = preferences.editor(env);
    preferences.put(env, editor, key, &[]);
    preferences.put(env, editor, other, &other_units);
    env.call_void(editor, preferences.apply, &[]);
    env.delete_local_ref(editor);
    assert_eq!(balanced_read(env), Ok(None));
    assert!(preferences.contains(env, key), "null Settings must preserve an existing empty key");
    assert_eq!(preferences.get(env, key), Some(Vec::new()));
    assert_eq!(preferences.get(env, other), Some(other_units.clone()));
    let persisted: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["strings"]["android_id"], "");
    assert_eq!(persisted["strings"]["android-id-fixture-other"].as_str().unwrap().encode_utf16().collect::<Vec<_>>(), other_units);

    let cached: Vec<u16> = "synthetic\0中😀尾".encode_utf16().collect();
    let editor = preferences.editor(env);
    preferences.put(env, editor, key, &cached);
    assert!(env.call_boolean(editor, preferences.commit, &[]));
    env.delete_local_ref(editor);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(balanced_read(env), Ok(Some(cached.clone())));
    assert_eq!(preferences.get(env, key), Some(cached));
    assert_eq!(std::fs::read(&path).unwrap(), before, "cached read must not change either synthetic key");

    env.set_static_object_field(player, field, std::ptr::null_mut());
    assert_eq!(balanced_read(env), Err(android_id::Error::PublicObject));
    drop(restore);
    let restored = env.get_static_object_field(player, field);
    assert!(env.is_same_object(restored, activity) && !env.exception_check());

    let error = env.find_class(c"java/lang/IllegalStateException");
    assert_eq!(env.throw_new(error, c"android-id synthetic pending throwable"), 0);
    let pending_before = env.exception_occurred();
    let result = android_id::read(env);
    let remained_pending = env.exception_check();
    let pending_after = env.exception_occurred();
    env.exception_clear();
    assert_eq!(result, Err(android_id::Error::JavaException));
    assert!(remained_pending && !pending_before.is_null());
    assert!(env.is_same_object(pending_before, pending_after), "source must preserve the same pending throwable");
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let editor = preferences.editor(env);
    for key in [key, other] {
        let returned = env.call_object(editor, preferences.remove, &[Arg::object(key)]);
        env.delete_local_ref(returned);
    }
    assert!(env.call_boolean(editor, preferences.commit, &[]));
    assert!(!env.exception_check());
    println!("Android ID source: real VM UTF-16 cache/null commit, key preservation, frame balance and pending throwable passed");
}
