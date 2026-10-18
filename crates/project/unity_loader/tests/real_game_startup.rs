//! INPUT:  locally supplied Vampire Survivors ARM64 data, production kit, Xvfb/Mesa
//! OUTPUT:  standard notification/unload acceptance and repeated memory/frame work A/B evidence
//! POS:    opt-in title acceptance; menus/blank frames/failed initialization cannot pass

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl)))]
#[test]
#[ignore = "requires local ARM64 game data and the Linux ARM64 SDL kit"]
fn real_vampire_survivors_startup() {
    panic!("use docker-rust.sh test --release --sdl on Linux ARM64");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
#[test]
#[ignore = "opt-in real game acceptance requires locally supplied original APKs and native libraries"]
fn real_vampire_survivors_startup() {
    run_real_game("ample");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
#[test]
#[ignore = "opt-in real game: pressure-triggered resource unload and managed GC"]
fn real_vampire_survivors_memory_pressure() {
    run_real_game("pressure");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
#[test]
#[ignore = "opt-in real game: resource unload without additional managed GC"]
fn real_vampire_survivors_memory_pressure_no_gc() {
    run_real_game("pressure-no-gc");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
#[test]
#[ignore = "opt-in real game: ample RAM, developer notification only, and default no-extra-GC fallback"]
fn real_vampire_survivors_standard_memory_chain() {
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let folder = format!("standard-memory/{run_id}");
    println!("REAL_GAME_STANDARD_EVIDENCE target/real-game-evidence/{folder}");
    for case in ["ample", "notify-only", "pressure-default"] {
        run_real_game_at(case, &format!("{folder}/{case}"));
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
#[test]
#[ignore = "opt-in real game performance experiment: warmup plus nine counterbalanced runs"]
fn real_vampire_survivors_memory_ab() {
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let folder = format!("memory-ab/{run_id}");
    println!("REAL_GAME_AB_EVIDENCE target/real-game-evidence/{folder}");
    // One unscored run primes the persistent driver cache. Each scored case
    // starts a new child/context and isolated save/cache directory. Latin
    // square order gives every case each position once; no parallel games.
    run_real_game_at("off", &format!("{folder}/warmup"));
    for (round, order) in [
        ["off", "pressure-no-gc", "pressure"],
        ["pressure-no-gc", "pressure", "off"],
        ["pressure", "off", "pressure-no-gc"],
    ].into_iter().enumerate() {
        for case in order {
            println!("REAL_GAME_AB_BEGIN round={} case={case}", round + 1);
            run_real_game_at(case, &format!("{folder}/round-{}-{case}", round + 1));
            println!("REAL_GAME_AB_END round={} case={case}", round + 1);
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
fn run_real_game(memory_case: &str) {
    run_real_game_at(memory_case, memory_case);
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
fn run_real_game_at(memory_case: &str, evidence_label: &str) {
    use rudroid_test_support::cargo::{
        assert_kit_features, crates_root, ensure_workspace_cdylib,
    };
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    assert_kit_features(platform_sdl_runtime::SDL_ENABLED);
    let game = std::env::var_os("RUDROID_VAMPIRE_GAME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crates_root().parent().unwrap().join(
            "dist/chuimi-vs-install-20260910/staging-verified/vampiresurvivors114/vampiresurvivors114/gamedata",
        ));
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        let path = game.join("lib/arm64-v8a").join(name);
        assert!(
            path.is_file(),
            "real game input missing: {}",
            path.display()
        );
    }
    for name in ["base.apk", "split_UnityDataAssetPack.apk"] {
        assert!(
            game.join(name).is_file(),
            "original APK input missing: {name}"
        );
    }
    let root = std::env::temp_dir().join(format!("rudroid-real-vampire-{memory_case}-{}", std::process::id()));
    let original = root.join("original-package");
    fs::create_dir_all(&original).unwrap();
    fs::copy(game.join("base.apk"), original.join("base.apk")).unwrap();
    std::os::unix::fs::symlink(
        game.join("split_UnityDataAssetPack.apk"),
        original.join("split_UnityDataAssetPack.apk"),
    )
    .unwrap();
    std::os::unix::fs::symlink(game.join("lib"), original.join("lib")).unwrap();
    let extracted=Command::new("python3").arg("-c").arg(r#"
import sys,zipfile,pathlib
source=pathlib.Path(sys.argv[1]);dest=pathlib.Path(sys.argv[2])
for apk,base in [('base.apk',dest),('split_UnityDataAssetPack.apk',dest/'assetpacks'/'UnityDataAssetPack')]:
 with zipfile.ZipFile(source/apk) as package:
  for member in package.infolist():
   name=pathlib.PurePosixPath(member.filename)
   if not member.is_dir() and name.parts[0]=='assets' and '..' not in name.parts:
    target=base.joinpath(*name.parts);target.parent.mkdir(parents=True,exist_ok=True)
    with package.open(member) as src,target.open('wb') as out:
     import shutil;shutil.copyfileobj(src,out,1024*1024)
pack=dest/'assetpacks'/'UnityDataAssetPack'/'assets'
for name in ['aa','EOS','UnityServicesProjectConfiguration.json']:
 if (pack/name).exists() and not (dest/'assets'/name).exists():(dest/'assets'/name).symlink_to(pack/name,target_is_directory=(pack/name).is_dir())
"#).arg(&game).arg(&original).status().expect("Python 3 unpacks original APK assets in isolated test directory");
    assert!(extracted.success());
    let kit = root.join("kit");
    fs::create_dir_all(&kit).unwrap();
    for (package, artifact) in [
        ("rudroid-android-framework", "android_base"),
        ("rudroid-android-addon-fmod", "sdk_fmod"),
        ("rudroid-android-addon-gms", "sdk_google_gms"),
        ("rudroid-android-addon-levelplay", "sdk_levelplay"),
        ("rudroid-unity-il2cpp-hook", "il2cpp_patch"),
        (
            "rudroid-unity-gamepatch-vampire-survivors",
            "vampire_survivors",
        ),
        ("rudroid-android-elf-load", "android_elf_load"),
        ("rudroid-android-bionic", "android_bionic"),
        ("rudroid-android-media-fallback", "android_media_fallback"),
        ("rudroid-android-zlib", "android_zlib"),
        ("rudroid-android-native-events", "android_native_events"),
        ("rudroid-android-jni-vm", "android_jni_vm"),
        ("rudroid-linux-log-stderr", "platform_log_stderr"),
        ("rudroid-linux-sdl-runtime", "platform_sdl_runtime"),
        (
            "rudroid-android-reply-catalog",
            "rudroid_android_reply_catalog",
        ),
        ("rudroid-unity-profile-6", "unity_6"),
        ("rudroid-unity-memory-policy", "unity_memory_policy"),
    ] {
        let library = ensure_workspace_cdylib(package, artifact);
        let name = if artifact == "rudroid_android_reply_catalog" {
            "libreply_catalog.so".into()
        } else {
            library.file_name().unwrap().to_owned()
        };
        fs::copy(library, kit.join(name)).unwrap();
    }
    // Report ALL relocation-referenced libc tuples before the child hits its
    // first error. This is a provider-address audit, not guest execution or
    // proof that the accepted functions suffice for a playable Unity title.
    unsafe {
        use rudroid_common::symbols::elf_exec::VersionedSymbolSource;
        use rudroid_common::symbols::elf_load::{ImportVisitor, VISIT_IMPORTS};
        use std::collections::BTreeMap;
        use std::ffi::{c_char, c_void, CStr, CString};
        struct Inventory {
            query: VersionedSymbolSource,
            accepted: usize,
            missing: BTreeMap<(String, String, bool), Vec<String>>,
            foreign: BTreeMap<(String, String, bool), Vec<String>>,
        }
        unsafe extern "C" fn collect(
            name: *const c_char,
            library: *const c_char,
            version: *const c_char,
            flags: u32,
            user: *mut c_void,
        ) {
            let inventory = &mut *user.cast::<Inventory>();
            if library.is_null() {
                inventory
                    .foreign
                    .entry((
                        "<unversioned>".into(),
                        "<unversioned>".into(),
                        flags & 1 != 0,
                    ))
                    .or_default()
                    .push(CStr::from_ptr(name).to_string_lossy().into_owned());
            }
            if !library.is_null() && CStr::from_ptr(library) != c"libc.so" {
                inventory
                    .foreign
                    .entry((
                        CStr::from_ptr(library).to_string_lossy().into_owned(),
                        CStr::from_ptr(version).to_string_lossy().into_owned(),
                        flags & 1 != 0,
                    ))
                    .or_default()
                    .push(CStr::from_ptr(name).to_string_lossy().into_owned());
                return;
            }
            let address = if flags & 2 == 0 {
                (inventory.query)(name, library, version, std::ptr::null_mut())
            } else {
                0
            }; // TLS values are not function/data addresses
            if address != 0 {
                inventory.accepted += 1;
                return;
            }
            if library.is_null() {
                return;
            } // other owners supply unsupported bare imports
            let library = CStr::from_ptr(library).to_str().unwrap().to_owned();
            let version = CStr::from_ptr(version).to_str().unwrap().to_owned();
            let name = CStr::from_ptr(name).to_str().unwrap().to_owned();
            inventory
                .missing
                .entry((library, version, flags & 1 != 0))
                .or_default()
                .push(name);
        }
        let provider = libloading::Library::new(kit.join("libandroid_bionic.so")).unwrap();
        let query: libloading::Symbol<VersionedSymbolSource> =
            provider.get(b"rudroid_bionic_symbol").unwrap();
        let elf = libloading::Library::new(kit.join("libandroid_elf_load.so")).unwrap();
        let visit: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, Option<ImportVisitor>, *mut c_void) -> i32,
        > = elf.get(VISIT_IMPORTS.as_bytes()).unwrap();
        for module in [
            "libc++_shared.so",
            "libmain.so",
            "libunity.so",
            "libil2cpp.so",
        ] {
            let mut inventory = Inventory {
                query: *query,
                accepted: 0,
                missing: BTreeMap::new(),
                foreign: BTreeMap::new(),
            };
            let path =
                CString::new(game.join("lib/arm64-v8a").join(module).to_str().unwrap()).unwrap();
            assert_eq!(
                visit(
                    path.as_ptr(),
                    Some(collect),
                    (&mut inventory as *mut Inventory).cast()
                ),
                0,
                "complete ELF metadata inventory required for {module}"
            );
            let missing_count: usize = inventory.missing.values().map(Vec::len).sum();
            println!("BIONIC_ABI_INVENTORY module={module} accepted_addresses={} missing_tuples={missing_count} scope=address_only", inventory.accepted);
            for ((library, version, weak), mut names) in inventory.foreign {
                names.sort_unstable();
                println!("NATIVE_ABI_REQUIREMENTS module={module} library={library} version={version} weak={weak} names={}", names.join(","));
            }
            for ((library, version, weak), mut names) in inventory.missing {
                names.sort_unstable();
                println!("BIONIC_ABI_MISSING module={module} library={library} version={version} weak={weak} names={}", names.join(","));
            }
        }
    }
    // Deliberately high reserve forces the production pressure path using
    // actual host/cgroup measurements, without allocating until OOM or
    // changing the fixed Docker resource contract. It is not a device preset.
    let memory_config = match memory_case {
        "off" => "[unity_memory]\nenabled=false\n".to_owned(),
        "ample" => "[unity_memory]\nenabled=true\n".to_owned(),
        "pressure" | "pressure-no-gc" | "pressure-default" | "notify-only" => format!(
            "[unity_memory]\nenabled=true\nreserve_mib=8192\nrecovery_mib=12288\nwarmup_ms=45000\ncooldown_ms=300000\nnotify_low_memory=true\nunload_unused_assets={}\n{}",
            memory_case != "notify-only",
            match memory_case {
                "pressure" => "collect_managed=true\n",
                "pressure-no-gc" => "collect_managed=false\n",
                _ => "", // Exercise the production default, rather than specifying it.
            }),
        _ => panic!("unknown memory experiment"),
    };
    let config = root.join("host.toml");
    fs::write(
        &config,
        format!(
            "[paths]\ngame_files={:?}\nandroid_package_code={:?}\nandroid_files={:?}\nandroid_cache={:?}\n\
         [package]\npackageName=\"com.poncle.vampiresurvivors\"\nmainIntentBundle={{unity=\"-force-gfx-direct -force-gles30\"}}\n\
         [device]\ndisplayWidth=320\ndisplayHeight=240\n\
           [runtime]\nmemory_mode=\"performance\"\n[unity]\ntarget_fps=90\nrender_sleep_us=0\nsmoke_frame_limit=5400\nlog_frame_progress=true\nlog_input_events=true\n[jni]\ntrace_native_registration=true\ntrace_messages=true\n[audio]\nlog_pcm_evidence=true\n[game_patches.vampire_survivors]\nmissing_debug_shader=true\n",
            game.to_str().unwrap(),
            original.join("base.apk").to_str().unwrap(),
            root.join("files").to_str().unwrap(),
            root.join("cache").to_str().unwrap(),
        ),
    )
    .unwrap();
    use std::io::Write;
    File::options().append(true).open(&config).unwrap().write_all(memory_config.as_bytes()).unwrap();
    fs::create_dir_all(root.join("files")).unwrap();
    fs::create_dir_all(root.join("cache")).unwrap();
    let evidence = crates_root().join("target/real-game-evidence").join(evidence_label);
    fs::create_dir_all(&evidence).unwrap();
    fs::copy(&config, evidence.join("host.toml")).unwrap();
    let frame = evidence.join("vampire-survivors.ppm");
    fs::write(&frame, []).unwrap();
    let pcm = evidence.join("vampire-survivors.f32le");
    fs::write(&pcm, []).unwrap();
    for ms in [15000, 26000, 35000, 56000, 90000] {
        fs::write(
            evidence.join(format!("vampire-survivors-ms-{ms}.ppm")),
            [],
        )
        .unwrap();
    }
    let stdout_path = evidence.join("stdout.log");
    let stderr_path = evidence.join("stderr.log");
    // The warning/menu/character-confirm/stage-start transitions are not
    // frame-deterministic. Enter at 32/42s can stop at character selection
    // when startup is slower. Keep the original late start, add two missing
    // confirmations and keep moving throughout the measured window so an
    // idle character does not die and turn the workload into a result menu.
    // The same timeline is used for every case; image acceptance stays strict.
    let mut input_script = "10000:touch:0:160:180;10100:touch:1:160:180;12000:key:0:66:0;12100:key:1:66:0;14000:key:0:29:0;14100:key:1:29:0;16000:key:0:62:0;16100:key:1:62:0;22000:touch:0:160:150;22100:touch:1:160:150;32000:key:0:66:0;32100:key:1:66:0;42000:key:0:66:0;42100:key:1:66:0;44000:key:0:66:0;44100:key:1:66:0;46000:key:0:66:0;46100:key:1:66:0".to_owned();
    // Accept upgrades instead of measuring a paused level-up menu. The late
    // confirmation train starts only after the intended stage-start time.
    for ms in (48_000..=96_000).step_by(2_000) {
        if ms == 52_000 { input_script.push_str(";52000:key:0:22:0"); }
        input_script.push_str(&format!(";{ms}:key:0:66:0;{}:key:1:66:0", ms + 100));
    }
    input_script.push_str(";98000:key:1:22:0");
    fs::write(evidence.join("input-script.txt"), &input_script).unwrap();
    // Full-time gameplay can be slower than the old menu-heavy probes on
    // software Mesa. Keep resource limits fixed and bound only this repeated
    // experiment at 210s; the ordinary startup cases retain their 150s cap.
    let timeout = Duration::from_secs(if evidence_label.starts_with("memory-ab/") { 210 } else { 150 });
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rudroid-unity-loader"))
        .arg(&kit)
        .arg("--config")
        .arg(&config)
        .current_dir(&game)
        .env_remove("RUDROID_SKIP_GUEST")
        .env("SDL_AUDIODRIVER","disk")
        .env("SDL_DISKAUDIOFILE",&pcm)
        .env("RUDROID_CAPTURE_GUEST_FRAME_PATH",&frame)
        .env("RUDROID_CAPTURE_GUEST_FRAME_AFTER_MS","50000")
        .env("RUDROID_CAPTURE_GUEST_FRAME_EXTRA_AFTER_MS","15000,26000,35000,56000,90000")
        .env("RUDROID_INPUT_SCRIPT", &input_script)
        .stdout(Stdio::from(File::create(&stdout_path).unwrap()))
        .stderr(Stdio::from(File::create(&stderr_path).unwrap()))
        .spawn()
        .expect("spawn the production Rust loader");
    let mut rss_kib = 0u64;
    let mut timed_out = false;
    let mut next_memory_sample = Duration::ZERO;
    let mut memory_samples = Vec::new();
    let status = loop {
        // VmHWM is the child's kernel high-water mark at this sample;
        // exited processes may disappear before the final read. This is
        // observed child RSS, not PSS, a GPU budget or container memory.
        if let Ok(status) = fs::read_to_string(format!("/proc/{}/status", child.id())) {
            for line in status
                .lines()
                .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
            {
                if let Some(value) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    rss_kib = rss_kib.max(value);
                }
            }
        }
        if started.elapsed() >= next_memory_sample {
            // Parent-side, opt-in diagnostics only: no allocation hooks, guest
            // pauses or hot-frame work. Kernel rollup separates proportional
            // anon/file/shmem shares; it does not identify allocation owners or
            // account for GPU memory outside the process mappings.
            let elapsed = started.elapsed();
            next_memory_sample = elapsed + Duration::from_secs(10);
            match fs::read_to_string(format!("/proc/{}/smaps_rollup", child.id())) {
                Ok(rollup) => {
                    let mut fields = Vec::new();
                    for line in rollup.lines() {
                        let Some((name, value)) = line.split_once(':') else { continue; };
                        if matches!(name, "Rss" | "Pss" | "Pss_Anon" | "Pss_File" | "Pss_Shmem" |
                            "Private_Clean" | "Private_Dirty" | "Anonymous" | "Swap" | "SwapPss") {
                            // Preserve native units and missing fields instead
                            // of fabricating zero on older kernels.
                            fields.push(format!("{name}={}", value.trim()));
                        }
                    }
                    let sample = format!("REAL_GAME_MEMORY child_elapsed_ms={} {}", elapsed.as_millis(), fields.join(";"));
                    println!("{sample}"); memory_samples.push(sample);
                }
                Err(error) => println!("REAL_GAME_MEMORY child_elapsed_ms={} unavailable={error}", elapsed.as_millis()),
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > timeout {
            timed_out = true;
            child.kill().expect("stop only the timed-out child loader");
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    fs::write(evidence.join("memory-rollup.txt"), memory_samples.join("\n")).unwrap();
    let elapsed_ms = started.elapsed().as_millis();
    let stdout = fs::read_to_string(stdout_path).unwrap();
    // Native crash records stay in the raw file for the owner. This test
    // only examines ordinary runtime and managed/JNI error messages.
    let stderr = BufReader::new(File::open(stderr_path).unwrap())
        .lines()
        .map(|line| line.expect("runtime text log"))
        .take_while(|line| !line.starts_with("[CRASH]"))
        .collect::<Vec<_>>()
        .join("\n");
    let result_line = format!("REAL_GAME_STARTUP case={memory_case} game=vampiresurvivors114 elapsed_ms={elapsed_ms} observed_child_rss_kib={rss_kib} timeout={timed_out} status={status}");
    fs::write(evidence.join("result.txt"), &result_line).unwrap();
    println!("{result_line}");
    println!("{stdout}\n{stderr}");
    assert!(
        !timed_out,
        "real game exceeded the bounded rendering probe"
    );
    assert!(
        status.success(),
        "real game startup failed; this is NOT game acceptance"
    );
    assert!(
        stdout.contains("result=started"),
        "the loader must complete the real profile lifecycle"
    );
    assert!(
        stderr.contains("optional missing debug shader fallback installed via metadata"),
        "the explicitly enabled title adaptation must install successfully"
    );
    assert!(
        !stderr.contains("TypeInitializationException")
            && !stderr.contains("Parameter name: shader"),
        "managed game initialization must not fail while frame swaps continue"
    );
    assert!(
        stderr.contains("nativeInjectEvent accepted=1")
            && stderr.contains("real PCM consumed:")
            && stderr.contains("cleanup completed"),
        "input, real audio and cleanup evidence required"
    );
    if memory_case == "off" {
        assert!(!stderr.contains("ready: metadata-validated UnloadUnusedAssets/isDone"));
        assert!(!stderr.contains("requested UnloadUnusedAssets"));
    } else {
        assert!(stderr.contains("ready: metadata-validated UnloadUnusedAssets/isDone"),
            "collector must bind the actual game's metadata; a registered-only switch is insufficient");
        assert!(stderr.contains("unity_version=6000.0.61f1"));
    }
    assert!(stderr.contains("Unity build GC-handle ABI verified: 6000.0.61f1; pointer-width adapter"),
        "title patch and collector must use the same verified exact-build table");
    assert!(stderr.contains("render work histogram: scope=all samples=5400"));
    assert!(stderr.contains("render work histogram: scope=45-90s samples="));
    if memory_case == "ample" {
        assert!(!stderr.contains("requested UnloadUnusedAssets"), "sufficient memory must not trigger cleanup");
        assert!(stderr.contains("player stop: requests=0 completed=0 pending=false"));
        assert!(stderr.contains("collect_managed=false"));
        assert!(!stderr.contains("notified low-memory notification="));
    } else if memory_case == "notify-only" {
        assert!(stderr.contains("notified low-memory notification=1 grace_ms=1000"));
        assert!(stderr.contains("notification outcome=NotificationOnly"));
        assert!(!stderr.contains("requested UnloadUnusedAssets"));
        assert!(stderr.contains("player stop: requests=0 completed=0 pending=false notifications=1 recovered=0 gc_runs=0 notification_waiting=false"));
    } else if memory_case != "off" {
        assert!(stderr.contains("notified low-memory notification=1 grace_ms=1000"));
        assert!(stderr.contains("requested UnloadUnusedAssets request=1"));
        assert!(stderr.contains("completed request=1"), "request alone is not cleanup completion");
        assert!(stderr.contains("player stop: requests=1 completed=1 pending=false"));
        assert!(!stderr.contains("collector disabled") && !stderr.contains("notification failed"));
        if memory_case == "pressure-default" || memory_case == "pressure-no-gc" {
            assert!(stderr.contains("gc_performed=false"));
            assert!(stderr.contains("notifications=1 recovered=0 gc_runs=0 notification_waiting=false"));
        }
        if memory_case == "pressure-default" { assert!(stderr.contains("collect_managed=false")); }
    }
    if memory_case != "off" {
        assert!(stderr.contains("host_swap_total_kib=") || memory_case == "ample");
        assert!(stderr.contains("swap_pss_kib=") || memory_case == "ample");
    }
    assert!(stderr.contains("[MEMORY] mode=performance"), "real loader must apply the requested memory mode");
    assert!(stderr.contains("render_sleep_us=0 target_fps=90"), "no implicit extra 16ms in the Unity6 render loop");
    let pixels = fs::read(&frame).unwrap();
    let header = b"P6\n320 240\n255\n";
    assert!(
        pixels.starts_with(header),
        "the actual guest must present gameplay after 50 seconds"
    );
    assert_eq!(
        pixels.len(),
        header.len() + 320 * 240 * 3,
        "complete guest pixel payload required"
    );
    let colors: std::collections::HashSet<_> = pixels[header.len()..].chunks_exact(3).collect();
    assert!(
        colors.len() > 16,
        "blank readback does not prove a visible game image"
    );
    assert!(stderr.lines().any(|line| line.contains("guest frame capture:") && line.contains("after_ms=50000") && line.contains("size=320x240 result=Ok(())")));
    assert!(
        stderr.contains("missing debug shader resolved with Sprites/Default"),
        "the original game's missing shader path must actually execute"
    );
    assert!(
        !stderr.contains("Exception setting up native Android input helper")
            && !stderr.contains("AndroidUnityInputHelper.OnDestroy"),
        "Rewired must initialize and release the shared InputManager"
    );
    let later = fs::read(evidence.join("vampire-survivors-ms-56000.ppm")).unwrap();
    let window_end = fs::read(evidence.join("vampire-survivors-ms-90000.ppm")).unwrap();
    for image in [&pixels, &later, &window_end] {
        assert!(image.starts_with(header));
        assert_eq!(image.len(), header.len() + 320 * 240 * 3);
        // This title/script selects Mad Forest. Its green field distinguishes
        // gameplay from the red character screen and black loading failure.
        let green = image[header.len()..]
            .chunks_exact(3)
            .filter(|rgb| {
                rgb[1] > 25
                    && u16::from(rgb[1]) * 10 > u16::from(rgb[0]) * 12
                    && u16::from(rgb[1]) * 10 > u16::from(rgb[2]) * 14
            })
            .count();
        assert!(green > 320 * 240 / 2, "actual Mad Forest gameplay required");
    }
    assert_ne!(
        pixels, later,
        "the game must continue advancing after movement input"
    );
    assert_ne!(later, window_end, "gameplay must advance to the end of the measured window");
    let samples = fs::read(pcm).unwrap();
    assert!(
        samples.len() >= 4096 && samples.len() % 8 == 0,
        "stereo float PCM required"
    );
    let peak = samples
        .chunks_exact(4)
        .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
        .filter(|s| s.is_finite())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        peak > 0.001,
        "audio must contain actual nonzero mixed samples"
    );
    println!("REAL_GAME_PLAYABLE native_render_calls=5400 target_fps=90 captures_after_ms=50000,56000,90000 input=accepted pcm_peak={peak} exit=clean environment=docker_mesa");
    fs::remove_dir_all(root).unwrap();
}
