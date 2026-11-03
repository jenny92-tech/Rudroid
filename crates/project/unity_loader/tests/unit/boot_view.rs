//! INPUT:  extracted synthetic boot files and validated startup projections
//! OUTPUT:  independent cursors, exact-file redirects, source preservation and owner cleanup checks
//! POS:    verifies the resource-file lifetime without executing Unity or native patches
use super::*;
use rudroid_host_config::HostConfig;

const BOOT: &str =
    "force-gfx-direct=1\ngfx-threading-mode=0\ngfx-disable-mt-rendering=1\ngc-max-time-slice=3\n";

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree() -> (Fixture, PathBuf) {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("rudroid-boot-fixture-{}-{id}", std::process::id()));
    std::fs::create_dir_all(root.join("bin/Data")).unwrap();
    let file = root.join("bin/Data/boot.config");
    std::fs::write(&file, BOOT).unwrap();
    (Fixture(root), file)
}

#[test]
fn private_boot_readers_own_independent_offsets_and_do_not_modify_source() {
    let (fixture, source) = tree();
    let p = HostConfig::parse(
        "[runtime]\nmemory_mode='performance'\n[unity]\nrender_thread_mode='memory_mode'\n",
    )
    .unwrap()
    .unity_startup_policy()
    .unwrap();
    let view = BootView::prepare(&source, &p).unwrap();
    let encoded = view
        .redirect(&CString::new(source.as_os_str().as_bytes()).unwrap())
        .unwrap();
    let path = PathBuf::from(OsStr::from_bytes(
        CStr::from_bytes_with_nul(&encoded).unwrap().to_bytes(),
    ));
    let mut a = File::open(&path).unwrap();
    let mut b = File::open(&path).unwrap();
    let meta = view._file.metadata().unwrap();
    let inode = (meta.dev(), meta.ino());
    let mut prefix = [0; 5];
    a.read_exact(&mut prefix).unwrap();
    let mut second = String::new();
    b.read_to_string(&mut second).unwrap();
    assert!(second.starts_with("force-gfx-direct=0\n"));
    assert!(second.ends_with("gc-max-time-slice=3\n"));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), BOOT);
    assert!(view
        .redirect(c"/assets/another/bin/Data/boot.config")
        .is_none());
    assert!(view.redirect(c"/proc/cpuinfo").is_none());
    // An already opened reader keeps its own file lease after the view drops.
    drop(view);
    let mut rest = String::new();
    a.read_to_string(&mut rest).unwrap();
    assert_eq!(String::from_utf8(prefix.to_vec()).unwrap() + &rest, second);
    drop(a);
    drop(b);
    // Concurrent tests may reuse the fd number, but cannot keep this unlinked inode.
    if let Ok(file) = File::open(&path) {
        let m = file.metadata().unwrap();
        assert_ne!((m.dev(), m.ino()), inode);
    }
    std::fs::remove_dir_all(&fixture.0).unwrap();
}

#[test]
fn rejected_boot_has_no_original_changes() {
    let (fixture, source) = tree();
    let p = HostConfig::parse("[unity]\nrender_thread_mode='multi'\n")
        .unwrap()
        .unity_startup_policy()
        .unwrap();
    std::fs::write(&source, "unknown=1\n").unwrap();
    assert!(BootView::prepare(&source, &p).is_err());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "unknown=1\n");
    std::fs::remove_dir_all(&fixture.0).unwrap();
}
