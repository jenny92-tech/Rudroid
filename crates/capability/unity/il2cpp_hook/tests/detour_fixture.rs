//! Ticket 51 acceptance: real ARM64 inline-code detours against
//! compiler-produced code pages.
//!
//! The fixture SO (`tests/fixtures/rudroid51_fixture.c`, compiled by this harness
//! with gcc) exports targets with distinct entry prologues plus a same-DSO
//! replacement. This binary is its own harness (harness = false) so the
//! parent can spawn ITSELF as a crash-isolated child per case:
//! - install before=65, after=66 (calls reach the replacement),
//! - via_trampoline=65 (the original stays callable while detoured),
//! - restored=65 (the saved prologue is back).
//!
//! Self-compiled fixtures only — this proves the detour mechanism, not any
//! real title. Android/Bionic and Silksong stay pending.
//!
//! Assembly contract (ticket 74, the 68/71 pattern): the elf_load cdylib is
//! located the way cargo itself does — CARGO_TARGET_DIR when the invocation
//! set one (relative paths resolve against the crates root), else the
//! workspace `target` — and rebuilt with THIS run's profile before dlopen,
//! so section 7 works from an empty target dir in either profile and can
//! never load a stale or foreign artifact.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let child = args.len() > 1 && args[1] == "--rudroid51-child";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    probe::run(child, args.get(2).map(|s| s.as_str()), args.get(3).map(|s| s.as_str()));
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
    {
        let _ = (child, args);
        eprintln!("rudroid51 detour fixture: skipped (requires linux/aarch64)");
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod probe {
    use std::ffi::{c_void, CString};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use il2cpp_patch::{HookRegistry, HookShape};

    // Verified against GNU as (objdump in the Docker log).
    const MOVZ_W0_66: u32 = 0x5280_0840;
    const RET: u32 = 0xD65F_03C0;

    /// B/BL immediate range in bytes (±128 MiB).
    const BRANCH_RANGE: i64 = (1 << 25) * 4;

    fn page_size() -> usize {
        // SAFETY: sysconf(_SC_PAGESIZE) is always safe.
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    /// dlopen the fixture and resolve its exports.
    fn load(so: &str) -> (usize, LibSymbols) {
        // SAFETY: RTLD_NOW|RTLD_LOCAL on a compiler-built dependency-free .so.
        let handle = unsafe {
            libc::dlopen(
                CString::new(so).unwrap().as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            )
        };
        assert!(!handle.is_null(), "dlopen {so} failed");
        let symbol = |name: &str| {
            // SAFETY: handle is live; NUL-free literal name.
            let address = unsafe {
                libc::dlsym(handle, CString::new(name).unwrap().as_ptr())
            };
            assert_ne!(address as usize, 0, "fixture must export {name}");
            address as usize
        };
        let symbols = LibSymbols {
            plain: symbol("rudroid51_hook_target_plain"),
            replacement: symbol("rudroid51_replacement_b"),
            nop: symbol("rudroid51_hook_target_nop"),
            stp: symbol("rudroid51_hook_target_stp"),
            tailb: symbol("rudroid51_hook_target_tailb"),
            adr: symbol("rudroid51_hook_target_adrp"),
        };
        (handle as usize, symbols)
    }

    struct LibSymbols {
        plain: usize,
        replacement: usize,
        nop: usize,
        stp: usize,
        tailb: usize,
        adr: usize,
    }

    fn registry_at(address: usize) -> HookRegistry {
        let mut registry = HookRegistry::default();
        registry.register_request("rudroid51", "fixture");
        registry.resolve("rudroid51", address).expect("resolve");
        registry
    }

    /// Locate (and rebuild) the ticket-49 elf_load cdylib the way cargo does
    /// (ticket 74, the 68/71 pattern): CARGO_TARGET_DIR when the invocation
    /// set one (relative paths resolve against the crates root), else the
    /// workspace `target`, always with this run's profile. `cargo test` does
    /// not produce cdylib artifacts, so the rebuild is what makes section 7
    /// work from an empty target dir.
    ///
    /// Freshness: cargo rebuilds on any input change, but it does NOT verify
    /// the content of an existing output (a file planted over the artifact —
    /// whatever its mtime — is "fresh" to cargo's fingerprint, which is how
    /// stale misloads happen). This fixture therefore additionally requires
    /// the artifact to be at least as new as the newest elf_load source; an
    /// older leftover fails with a precise message instead of being dlopened.
    /// (A deliberately forged newer-mtime file is an adversarial act outside
    /// the test's threat model, same as ticket 71's assembly.)
    fn ensure_elf_load_cdylib() -> std::path::PathBuf {
        use std::path::{Path, PathBuf};
        use std::process::Command;

        let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("crates root above capability/unity/il2cpp_hook")
            .to_owned();
        let package_root = crates_root.join("capability/android/elf_load");
        let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                if dir.is_absolute() {
                    dir
                } else {
                    crates_root.join(dir)
                }
            }
            None => crates_root.join("target"),
        };
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let lib = if cfg!(target_os = "macos") {
            "libandroid_elf_load.dylib"
        } else {
            "libandroid_elf_load.so"
        };
        let artifact = target_dir.join(profile).join(lib);
        let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        command.args(["build", "--locked", "-p", "rudroid-android-elf-load"]);
        if !cfg!(debug_assertions) {
            command.arg("--release");
        }
        let status = command.current_dir(&crates_root).status().expect("spawn cargo");
        assert!(status.success(), "building rudroid-android-elf-load failed");
        assert!(
            artifact.exists(),
            "cargo build succeeded but no elf_load cdylib at {}; \
             the fixture resolved a different CARGO_TARGET_DIR than the build",
            artifact.display()
        );
        // Newest source of the elf_load package (src tree + Cargo.toml).
        let mut source_times: Vec<std::time::SystemTime> =
            vec![mtime_of(&package_root.join("Cargo.toml"))];
        source_times.extend(glob_src_rs(&package_root).into_iter().map(|p| mtime_of(&p)));
        let newest_source = source_times
            .into_iter()
            .max()
            .expect("elf_load sources must exist");
        let built = mtime_of(&artifact);
        assert!(
            built >= newest_source,
            "stale elf_load cdylib at {} (older than the newest source); \
             refusing to dlopen a leftover — delete it and rebuild the workspace",
            artifact.display()
        );
        artifact
    }

    fn mtime_of(path: &std::path::Path) -> std::time::SystemTime {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    }

    fn glob_src_rs(package_root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![package_root.join("src")];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    found.push(path);
                }
            }
        }
        found
    }

    /// Dynamic binding to elf_load's C API (ticket 49 executable backend).
    struct ElfLoadApi {
        // Kept for documentation: the cdylib stays loaded for the process
        // lifetime (same lifetime model as the detour trampolines), so it is
        // never dlclosed.
        #[allow(dead_code)]
        handle: *mut c_void,
        exec_load: unsafe extern "C" fn(*const std::ffi::c_char) -> *mut c_void,
        exec_symbol: unsafe extern "C" fn(*const c_void, *const std::ffi::c_char) -> usize,
        exec_destroy: unsafe extern "C" fn(*mut c_void),
    }

    impl ElfLoadApi {
        fn load(path: &str) -> Option<Self> {
            // SAFETY: dlopen of a workspace-built cdylib path.
            let handle = unsafe {
                libc::dlopen(
                    CString::new(path.to_owned()).ok()?.as_ptr(),
                    libc::RTLD_NOW | libc::RTLD_LOCAL,
                )
            };
            if handle.is_null() {
                return None;
            }
            // SAFETY: handle is live; symbol names are NUL-free literals.
            unsafe {
                Some(Self {
                    handle,
                    exec_load: std::mem::transmute(libc::dlsym(
                        handle,
                        c"rudroid_elf_exec_load".as_ptr(),
                    )),
                    exec_symbol: std::mem::transmute(libc::dlsym(
                        handle,
                        c"rudroid_elf_exec_symbol".as_ptr(),
                    )),
                    exec_destroy: std::mem::transmute(libc::dlsym(
                        handle,
                        c"rudroid_elf_exec_destroy".as_ptr(),
                    )),
                })
            }
        }

        fn exec_load(&self, so: &str) -> Option<ElfModule<'_>> {
            // SAFETY: path is a valid NUL-terminated CString.
            let raw = unsafe {
                (self.exec_load)(CString::new(so.to_owned()).ok()?.as_ptr())
            };
            if raw.is_null() {
                return None;
            }
            Some(ElfModule { api: self, raw })
        }
    }

    struct ElfModule<'a> {
        api: &'a ElfLoadApi,
        raw: *mut c_void,
    }

    impl ElfModule<'_> {
        fn symbol(&self, name: &str) -> Option<usize> {
            // SAFETY: live module handle.
            let address = unsafe {
                (self.api.exec_symbol)(
                    self.raw,
                    CString::new(name.to_owned()).ok()?.as_ptr(),
                )
            };
            (address != 0).then_some(address)
        }
    }

    impl Drop for ElfModule<'_> {
        fn drop(&mut self) {
            // SAFETY: handle created by exec_load, destroyed once.
            unsafe { (self.api.exec_destroy)(self.raw) };
        }
    }

    fn call(address: usize) -> u32 {
        // SAFETY: address is a fixture entry returning a u32.
        let function: extern "C" fn() -> u32 = unsafe { std::mem::transmute(address) };
        function()
    }

    /// `/proc/<pid>/maps` ranges overlapping `[address, address)` as
    /// (start, end, perms).
    fn overlapping(address: usize) -> Vec<(usize, usize, String)> {
        let mut found = Vec::new();
        let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
            return found;
        };
        for line in maps.lines() {
            let Some((range, rest)) = line.split_once(' ') else {
                continue;
            };
            let perms = rest.split_whitespace().next().unwrap_or("").to_owned();
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            let (Ok(start), Ok(end)) =
                (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16))
            else {
                continue;
            };
            if start <= address && address < end {
                found.push((start, end, perms));
            }
        }
        found
    }

    fn assert_not_wx(address: usize, context: &str) {
        for (start, end, perms) in overlapping(address) {
            assert!(
                !(perms.contains('w') && perms.contains('x')),
                "{context}: range {start:#x}-{end:#x} covering {address:#x} must \
                 never be writable AND executable (perms={perms})"
            );
        }
    }

    /// The full three-state contract on one target:
    /// 65 (before) -> 66 (detoured) -> trampoline 65 -> restored 65.
    fn tristate(target: usize, destination: usize) -> (HookShape, u32, u32, u32) {
        assert_eq!(call(target), 65, "fixture must return 'A' before the hook");

        let registry = registry_at(target);
        // SAFETY: fixture code page; nothing executes the entry meanwhile.
        let detour = unsafe { registry.apply_detour("rudroid51", destination) }
            .expect("install on a compiler-produced entry");
        assert_eq!(detour.replacement, destination);
        assert_not_wx(target, "after install");

        let after = call(target);
        // SAFETY: trampoline replays the saved prologue.
        let via_trampoline = call(detour.original);

        // SAFETY: nothing executes the entry meanwhile.
        unsafe { registry.restore_detour(&detour) }.expect("restore");
        assert_not_wx(target, "after restore");
        (detour.shape, after, via_trampoline, call(target))
    }

    // -- child cases (crash-isolated) ------------------------------------

    fn child(case: &str, so: &str) -> i32 {
        let (_, symbols) = load(so);
        match case {
            // Near-range: the replacement lives in the same DSO.
            "tristate" => {
                let (shape, after, via, restored) =
                    tristate(symbols.plain, symbols.replacement);
                assert_eq!(shape, HookShape::Branch, "same-DSO must be a direct B");
                println!("rudroid51 tristate before=65 after={after} via_tramp={via} restored={restored} shape=branch");
            }
            // Far-range: destination on a page mmap'd ~16 TiB away -> veneer.
            "far" => {
                let (page, _) = far_destination_page();
                let distance = symbols.plain.abs_diff(page as usize) as i64;
                assert!(
                    distance > BRANCH_RANGE,
                    "test defect: distance {distance} must exceed the branch range"
                );
                let (shape, after, via, restored) = tristate(symbols.plain, page as usize);
                assert_eq!(shape, HookShape::AbsoluteJump, "out-of-range needs the veneer");
                println!(
                    "rudroid51 far before=65 after={after} via_tramp={via} restored={restored} shape=veneer distance_mb={}",
                    distance / (1024 * 1024)
                );
            }
            // NOP and STP prologue shapes through the same contract.
            "nop-stp" => {
                for (name, target) in [("nop", symbols.nop), ("stp", symbols.stp)] {
                    let (shape, after, via, restored) =
                        tristate(target, symbols.replacement);
                    assert_eq!(shape, HookShape::Branch);
                    println!("rudroid51 {name} before=65 after={after} via_tramp={via} restored={restored}");
                }
            }
            // The entry IS an unconditional B: the trampoline re-emits it as
            // an absolute jump to the tail target.
            "tailb" => {
                let (shape, after, via, restored) =
                    tristate(symbols.tailb, symbols.replacement);
                assert_eq!(shape, HookShape::Branch);
                println!("rudroid51 tailb before=65 after={after} via_tramp={via} restored={restored}");
            }
            // ADRP entry (the real global-access prologue): the trampoline
            // re-materializes the page address into x1, so it still loads
            // the global (65) — oracle `bd_emit_orig` parity.
            "adrp" => {
                // SAFETY: fixture entry; the first word is the ADRP.
                let entry = unsafe { (symbols.adr as *const u32).read_volatile() };
                assert_eq!(entry & 0x9F00_0000, 0x9000_0000, "the ADRP word");
                let (shape, after, via, restored) = tristate(symbols.adr, symbols.replacement);
                assert_eq!(shape, HookShape::Branch);
                println!("rudroid51 adrp before=65 after={after} via_tramp={via} restored={restored}");
            }
            other => {
                eprintln!("unknown child case {other}");
                return 2;
            }
        }
        0
    }

    /// A destination page mmap'd near 16 TiB — far beyond ±128 MiB from any
    /// loaded image. Written once before its first execution, so no stale
    /// I-cache lines exist (same argument as elf_load exec.rs, ticket 49).
    fn far_destination_page() -> (*mut u8, usize) {
        let page = page_size();
        let hint = 0x0000_1000_0000_0000usize;
        // SAFETY: anonymous mmap with a hint; failure is MAP_FAILED.
        let map = unsafe {
            libc::mmap(
                hint as *mut c_void,
                page,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap far destination page");
        let bytes: Vec<u8> = [MOVZ_W0_66, RET].iter().flat_map(|w| w.to_le_bytes()).collect();
        // SAFETY: the page is `page` bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), map as *mut u8, bytes.len()) };
        // SAFETY: our own page.
        let rc = unsafe { libc::mprotect(map, page, libc::PROT_READ | libc::PROT_EXEC) };
        assert_eq!(rc, 0, "mprotect far destination RX");
        (map as *mut u8, page)
    }

    // -- parent -----------------------------------------------------------

    /// Build `tests/fixtures/rudroid51_fixture.c` with the system C compiler into a
    /// per-process temp dir. A missing compiler or a
    /// failed build fails the harness — it never passes by skipping.
    fn compile_fixture() -> String {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rudroid51_fixture.c");
        let out_dir = std::env::temp_dir().join(format!("rudroid51-{}", std::process::id()));
        std::fs::create_dir_all(&out_dir).expect("fixture output dir");
        let so = out_dir.join("rudroid51_fixture.so");
        let status = std::process::Command::new("gcc")
            .args(["-shared", "-fPIC", "-O0", "-o"])
            .arg(&so)
            .arg(&source)
            .status()
            .expect("the detour fixture needs gcc");
        assert!(status.success(), "compiling {} failed", source.display());
        so.to_str().expect("utf8 fixture path").to_owned()
    }

    fn spawn(exe: &std::path::Path, case: &str, so: &str) -> std::process::Output {
        std::process::Command::new(exe)
            .arg("--rudroid51-child")
            .arg(case)
            .arg(so)
            .output()
            .expect("spawn child")
    }

    fn expect_child(exe: &std::path::Path, case: &str, so: &str, needle: &str) {
        let out = spawn(exe, case, so);
        eprintln!(
            "[{case}] status: {}\nstdout:\n{}stderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "child case {case} must pass");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(needle),
            "child case {case} must report `{needle}`, got: {stdout}"
        );
    }

    pub fn run(is_child: bool, case: Option<&str>, so: Option<&str>) {
        if is_child {
            let case = case.expect("child case argument");
            let so = so.expect("child RUDROID51_SO argument");
            std::process::exit(child(case, so));
        }

        let so = compile_fixture();

        let exe = std::env::current_exe().unwrap();

        // 1. Near-range tristate on the plain prologue (direct B shape).
        expect_child(
            &exe,
            "tristate",
            &so,
            "after=66 via_tramp=65 restored=65 shape=branch",
        );

        // 2. Far-range destination -> absolute veneer shape.
        expect_child(
            &exe,
            "far",
            &so,
            "after=66 via_tramp=65 restored=65 shape=veneer",
        );

        // 3. NOP and STP/LDP prologue shapes.
        expect_child(
            &exe,
            "nop-stp",
            &so,
            "rudroid51 nop before=65 after=66 via_tramp=65 restored=65",
        );
        expect_child(
            &exe,
            "nop-stp",
            &so,
            "rudroid51 stp before=65 after=66 via_tramp=65 restored=65",
        );

        // 4. Entry-is-a-tail-branch shape.
        expect_child(&exe, "tailb", &so, "after=66 via_tramp=65 restored=65");

        // 5. ADRP entry: re-materialized into the same register.
        expect_child(&exe, "adrp", &so, "rudroid51 adrp before=65 after=66 via_tramp=65 restored=65");

        // 6. Execute permission under a concurrent executor: repeated
        //    install/restore cycles on `plain` while another thread keeps
        //    calling `nop` (same text page) and sampling /proc/self/maps.
        //    The patch window is RWX (oracle parity), never execute-less —
        //    a window without +x faults the executor thread.
        let (_, symbols) = load(&so);
        let destination = symbols.replacement;
        let page = page_size();
        assert_eq!(
            symbols.plain / page,
            symbols.nop / page,
            "test defect: the executor must share the patched page"
        );
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<String>();
        let observer = {
            let stop = std::sync::Arc::clone(&stop);
            let sibling = symbols.nop;
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    assert_eq!(call(sibling), 65, "the same-page sibling keeps executing");
                    for (start, end, perms) in overlapping(sibling) {
                        if !perms.contains('x') {
                            tx.send(format!("{start:#x}-{end:#x} {perms}")).ok();
                        }
                    }
                }
            })
        };
        for _ in 0..50 {
            let (_, _, _, restored) = tristate(symbols.plain, destination);
            assert_eq!(restored, 65);
        }
        stop.store(true, Ordering::Relaxed);
        observer.join().expect("the same-page executor never faulted");
        let lost = rx.into_iter().collect::<Vec<_>>();
        assert!(
            lost.is_empty(),
            "the patched page must never lose execute permission: {lost:?}"
        );

        // 7. elf_load integration (dynamic, ticket-49 backend): dlopen the
        //    libandroid_elf_load.so cdylib and map the fixture through its C
        //    API — the mapped module must be hookable through the same
        //    detour surface. Capability crates are never statically linked
        //    together (ticket-43 boundary), so this mirrors deployment.
        let elf_so = ensure_elf_load_cdylib()
            .to_str()
            .expect("utf8 elf_load cdylib path")
            .to_owned();
        let api = ElfLoadApi::load(&elf_so).expect("dlopen the ticket-49 elf_load cdylib");
        let module = api
            .exec_load(&so)
            .expect("elf_load maps the fixture");
        let target = module
            .symbol("rudroid51_hook_target_plain")
            .expect("elf_load resolves the fixture export");
        assert_eq!(call(target), 65, "mapped guest code executes before the hook");
        let (shape, after, via, restored) = tristate(target, destination);
        assert_eq!(shape, HookShape::Branch);
        assert_eq!((after, via, restored), (66, 65, 65));
        println!("rudroid51 elfload before=65 after={after} via_tramp={via} restored={restored} shape=branch");

        println!("rudroid51-fixture: all probes complete (real inline detours)");
    }
}
