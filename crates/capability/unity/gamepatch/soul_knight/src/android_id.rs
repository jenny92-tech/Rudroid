//! INPUT: attached shared JNI environment and the configured public Android Context
//! OUTPUT: the original nullable Android ID cache/Settings result, as exact UTF-16
//! POS: title-private platform source; no SDK lifecycle, authentication or hooks
#![forbid(unsafe_code)]

use std::ffi::CStr;

use rudroid_common::jni_env::{Arg, JObject, JniEnv, Method};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    JavaException,
    LocalFrame,
    PublicContract,
    PublicObject,
    StringCopy,
}

/// Reproduce BasePlayerActivity.getAndroidID using only public Android calls.
/// The caller must supply its own current-thread environment and must already
/// have selected the isolated owned profile. This module neither attaches a
/// thread nor verifies mode/profile ownership. Any JNI exception stays pending;
/// null is a valid Settings result, never an ID to invent or an error to hide.
pub(super) fn read(env: JniEnv) -> Result<Option<Vec<u16>>, Error> {
    let _locals = LocalFrame::enter(env)?;
    let mut source = Android::new(env)?;
    read_from(&mut source).map(|value| value.map(|text| text.units))
}

// This private seam isolates the original cache policy from JNI reference and
// descriptor mechanics. Tests use synthetic text; callers see only read().
trait Source {
    type Text: AsRef<[u16]>;
    fn cached(&mut self) -> Result<Option<Self::Text>, Error>;
    fn settings(&mut self) -> Result<Option<Self::Text>, Error>;
    fn edit(&mut self) -> Result<(), Error>;
    fn put(&mut self, text: &Self::Text) -> Result<(), Error>;
    fn commit(&mut self) -> Result<bool, Error>;
}

fn read_from<S: Source>(source: &mut S) -> Result<Option<S::Text>, Error> {
    if let Some(cached) = source.cached()? {
        if !cached.as_ref().is_empty() {
            return Ok(Some(cached));
        }
    }
    let value = source.settings()?;
    source.edit()?;
    // Original SPUtil.put(null) still commits, but never calls putString(null):
    // Android interprets that call as deleting the key, which is different.
    if let Some(text) = &value {
        source.put(text)?;
    }
    let _ = source.commit()?; // Original code ignores false, not exceptions.
    Ok(value)
}

struct LocalFrame(JniEnv);

impl LocalFrame {
    fn enter(env: JniEnv) -> Result<Self, Error> {
        checked(env)?;
        let result = env.push_local_frame(32);
        if result != 0 {
            checked(env)?;
            return Err(Error::LocalFrame);
        }
        let frame = Self(env);
        checked(env)?;
        Ok(frame)
    }
}

impl Drop for LocalFrame {
    fn drop(&mut self) {
        // PopLocalFrame is legal with a pending exception. No Java local
        // escapes: read() copies UTF-16 and this runs on every return/unwind.
        self.0.pop_local_frame(std::ptr::null_mut());
    }
}

fn checked(env: JniEnv) -> Result<(), Error> {
    if env.exception_check() { Err(Error::JavaException) } else { Ok(()) }
}

struct Text {
    object: JObject,
    units: Vec<u16>,
}

impl AsRef<[u16]> for Text {
    fn as_ref(&self) -> &[u16] { &self.units }
}

struct Android {
    env: JniEnv,
    context: JObject,
    context_class: JObject,
    preferences: JObject,
    preferences_class: JObject,
    key: JObject,
    editor: JObject,
    editor_class: JObject,
}

impl Android {
    fn new(env: JniEnv) -> Result<Self, Error> {
        let mut source = Self {
            env, context: std::ptr::null_mut(), context_class: std::ptr::null_mut(),
            preferences: std::ptr::null_mut(), preferences_class: std::ptr::null_mut(),
            key: std::ptr::null_mut(), editor: std::ptr::null_mut(),
            editor_class: std::ptr::null_mut(),
        };
        let player = source.class(c"com/unity3d/player/UnityPlayer")?;
        let field = env.get_static_field_id(player, c"currentActivity", c"Landroid/app/Activity;");
        checked(env)?;
        if field.is_null() { return Err(Error::PublicContract); }
        source.context = source.required(env.get_static_object_field(player, field))?;
        source.context_class = source.class(c"android/content/Context")?;
        source.preferences_class = source.class(c"android/content/SharedPreferences")?;
        let name = source.required(env.new_string_utf(c"share_data"))?;
        source.key = source.required(env.new_string_utf(c"android_id"))?;
        let get = source.method(source.context_class, c"getSharedPreferences",
            c"(Ljava/lang/String;I)Landroid/content/SharedPreferences;")?;
        source.preferences = source.required(env.call_object(source.context, get,
            &[Arg::object(name), Arg::int(0)]))?;
        Ok(source)
    }

    fn required(&self, object: JObject) -> Result<JObject, Error> {
        checked(self.env)?;
        if object.is_null() { Err(Error::PublicObject) } else { Ok(object) }
    }

    fn class(&self, name: &CStr) -> Result<JObject, Error> {
        self.required(self.env.find_class(name))
    }

    fn method(&self, class: JObject, name: &CStr, signature: &CStr) -> Result<Method, Error> {
        let method = self.env.method(class, name, signature);
        checked(self.env)?;
        method.ok_or(Error::PublicContract)
    }

    fn text(&self, object: JObject) -> Result<Option<Text>, Error> {
        checked(self.env)?;
        if object.is_null() { return Ok(None); }
        let units = self.env.string_utf16(object);
        checked(self.env)?;
        units.map(|units| Some(Text { object, units })).ok_or(Error::StringCopy)
    }
}

impl Source for Android {
    type Text = Text;

    fn cached(&mut self) -> Result<Option<Text>, Error> {
        let default = self.required(self.env.new_string_utf(c""))?;
        let get = self.method(self.preferences_class, c"getString",
            c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;")?;
        self.text(self.env.call_object(self.preferences, get,
            &[Arg::object(self.key), Arg::object(default)]))
    }

    fn settings(&mut self) -> Result<Option<Text>, Error> {
        let get_resolver = self.method(self.context_class, c"getContentResolver",
            c"()Landroid/content/ContentResolver;")?;
        let resolver = self.required(self.env.call_object(self.context, get_resolver, &[]))?;
        let secure = self.class(c"android/provider/Settings$Secure")?;
        let get = self.env.static_method(secure, c"getString",
            c"(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;");
        checked(self.env)?;
        let get = get.ok_or(Error::PublicContract)?;
        self.text(self.env.call_static_object(secure, get,
            &[Arg::object(resolver), Arg::object(self.key)]))
    }

    fn edit(&mut self) -> Result<(), Error> {
        let edit = self.method(self.preferences_class, c"edit",
            c"()Landroid/content/SharedPreferences$Editor;")?;
        self.editor = self.required(self.env.call_object(self.preferences, edit, &[]))?;
        self.editor_class = self.class(c"android/content/SharedPreferences$Editor")?;
        Ok(())
    }

    fn put(&mut self, text: &Text) -> Result<(), Error> {
        let put = self.method(self.editor_class, c"putString",
            c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;")?;
        self.env.call_object(self.editor, put,
            &[Arg::object(self.key), Arg::object(text.object)]);
        checked(self.env)
    }

    fn commit(&mut self) -> Result<bool, Error> {
        let commit = self.method(self.editor_class, c"commit", c"()Z")?;
        let result = self.env.call_boolean(self.editor, commit, &[]);
        checked(self.env)?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../tests/unit/android_id.rs"]
mod tests;
