use super::*;

#[derive(Default)]
struct Profile {
    cache: Option<Vec<u16>>,
    setting: Option<Vec<u16>>,
    written: Option<Vec<u16>>,
    calls: Vec<&'static str>,
    fail: Option<&'static str>,
    commit_result: bool,
}

impl Profile {
    fn call(&mut self, name: &'static str) -> Result<(), Error> {
        self.calls.push(name);
        if self.fail == Some(name) { Err(Error::JavaException) } else { Ok(()) }
    }
}

impl Source for Profile {
    type Text = Vec<u16>;
    fn cached(&mut self) -> Result<Option<Self::Text>, Error> {
        self.call("cache")?;
        Ok(self.cache.clone())
    }
    fn settings(&mut self) -> Result<Option<Self::Text>, Error> {
        self.call("settings")?;
        Ok(self.setting.clone())
    }
    fn edit(&mut self) -> Result<(), Error> { self.call("edit") }
    fn put(&mut self, text: &Self::Text) -> Result<(), Error> {
        self.call("put")?;
        self.written = Some(text.clone());
        Ok(())
    }
    fn commit(&mut self) -> Result<bool, Error> {
        self.call("commit")?;
        Ok(self.commit_result)
    }
}

#[test]
fn nonempty_cache_returns_exact_utf16_without_settings_or_writes() {
    // Synthetic data includes NUL and an unpaired surrogate. The platform
    // source must never apply Rust's lossy String conversion to Java text.
    let cached = vec![0x61, 0, 0xd800];
    let mut profile = Profile { cache: Some(cached.clone()), ..Profile::default() };
    assert_eq!(read_from(&mut profile), Ok(Some(cached)));
    assert_eq!(profile.calls, ["cache"]);
    assert_eq!(profile.written, None);
}

#[test]
fn empty_or_null_cache_queries_settings_and_commits_nonnull_text() {
    for cache in [None, Some(Vec::new())] {
        for setting in [Vec::new(), vec![0, 0xdfff, 0x62]] {
            let mut profile = Profile { cache: cache.clone(), setting: Some(setting.clone()), ..Profile::default() };
            assert_eq!(read_from(&mut profile), Ok(Some(setting.clone())));
            assert_eq!(profile.calls, ["cache", "settings", "edit", "put", "commit"]);
            assert_eq!(profile.written, Some(setting));
        }
    }
}

#[test]
fn null_settings_still_edits_and_commits_without_put_or_remove() {
    for cache in [None, Some(Vec::new())] {
        let mut profile = Profile { cache: cache.clone(), ..Profile::default() };
        assert_eq!(read_from(&mut profile), Ok(None));
        assert_eq!(profile.calls, ["cache", "settings", "edit", "commit"]);
        assert_eq!(profile.cache, cache);
        assert_eq!(profile.written, None);
    }
}

#[test]
fn commit_false_is_ignored_as_in_original_but_exceptions_stop_later_calls() {
    let calls = ["cache", "settings", "edit", "put", "commit"];
    for (index, fail) in calls.into_iter().enumerate() {
        let mut profile = Profile {
            setting: Some(vec![0x61]), fail: Some(fail), ..Profile::default()
        };
        assert_eq!(read_from(&mut profile), Err(Error::JavaException));
        assert_eq!(profile.calls, calls[..=index]);
    }
    let mut profile = Profile { setting: Some(vec![0x61]), commit_result: false, ..Profile::default() };
    assert_eq!(read_from(&mut profile), Ok(Some(vec![0x61])));
    assert_eq!(profile.calls, calls);
}
