//! Shared helper for the exec tests: compile a `tests/fixtures/<stem>.c` fixture
//! into a shared object with the canonical container's `cc`, once per
//! process. A missing compiler fails the test loudly instead of skipping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Compile `tests/fixtures/<stem>.c` (plus `extra` flags) into `<out>.so` in a
/// per-process temp directory and return its path; repeated requests for
/// the same `out` reuse the first build.
pub fn compile(stem: &str, out: &str, extra: &[&str]) -> PathBuf {
    static BUILT: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);
    let mut built = BUILT.lock().unwrap_or_else(|p| p.into_inner());
    let built = built.get_or_insert_with(HashMap::new);
    if let Some(path) = built.get(out) {
        return path.clone();
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{stem}.c"));
    let dir = std::env::temp_dir().join(format!("rudroid-elf-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let output = dir.join(format!("{out}.so"));
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-nostdlib"])
        .args(extra)
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "cannot run `cc` to build {stem}.c: {error} — the canonical \
                 container (crates/scripts/docker-rust.sh) provides it"
            )
        });
    assert!(status.success(), "compiling {} failed", source.display());
    built.insert(out.to_owned(), output.clone());
    output
}

/// [`compile`] with the source stem as the output name and no extra flags.
#[allow(dead_code)]
pub fn fixture(stem: &str) -> String {
    compile(stem, stem, &[]).to_str().unwrap().to_owned()
}
