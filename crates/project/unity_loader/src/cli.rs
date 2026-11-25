//! INPUT:  CLI arguments, a native kit and Host Config
//! OUTPUT:  native lifecycle diagnostics and ExitCode
//! POS:    shared CLI body for rudroid-unity-loader and rudroid-loader
//! rudroid-unity-loader / rudroid-loader CLI (ticket 44).
//!
//! - **run** (default): install the Host Config for every plugin getter,
//!   bring up the capability kit through the three phases, map the guest SOs
//!   from `[paths].game_files` and run the Unity profile lifecycle
//!   (prepare → bootstrap → run, ticket 53). Any missing key/directory/SO,
//!   missing required guest function, wrong JNI return or failed phase
//!   exits non-zero naming the reason — a successful guest map without a
//!   real lifecycle is never a game start.
//! - **check** (`--check`, opt-in): descriptor load + topology validation
//!   only. Phases and guest bring-up do not run; the banner says so, so a
//!   check pass can never be presented as a game start.
//!
//! `RUDROID_SKIP_GUEST=1` downgrades a run to a plugin/phase smoke: it prints an
//! explicit SKIPPED banner and never counts as startup acceptance.
#![deny(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::process::ExitCode;

use rudroid_host_config::HostConfig;
use rudroid_unity_loader::{
    bring_up, guest, host_api, load_deploy_dir, runtime, GuestPlan, Selection,
};

const USAGE: &str =
    "usage: rudroid-unity-loader <deploy_dir> [--graphics-features | --probe-vulkan | --config <host-config.toml> [--check]]";

struct Args {
    deploy_dir: PathBuf,
    config_path: Option<PathBuf>,
    check: bool,
    graphics_features: bool,
    probe_vulkan: bool,
}

fn parse_args() -> Option<Args> {
    // Existing ports invoke unityloader <config>. Keep their entry/resources intact;
    // the matching Rust capabilities live in the adjacent rudroid.runtime directory.
    let executable = std::env::current_exe().ok()?;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if executable.file_name().is_some_and(|name|name=="unityloader")
        && (raw == ["--graphics-features"] || raw == ["--probe-vulkan"]) {
        return Some(Args { deploy_dir: executable.parent()?.join("rudroid.runtime"), config_path: None,
            check: false, graphics_features: raw[0]=="--graphics-features", probe_vulkan:raw[0]=="--probe-vulkan" });
    }
    if executable.file_name().is_some_and(|name| name == "unityloader")
        && (raw.len()==1 || (raw.len()==2 && raw[1]=="--check"))
        && !raw[0].starts_with('-') {
        return Some(Args { deploy_dir: executable.parent()?.join("rudroid.runtime"),
            config_path:Some(PathBuf::from(&raw[0])), check:raw.len()==2, graphics_features: false, probe_vulkan:false });
    }
    let mut deploy_dir: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut check = false;
    let mut graphics_features = false;
    let mut probe_vulkan = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => match args.next() {
                Some(path) => config_path = Some(PathBuf::from(path)),
                None => return None,
            },
            "--check" => check = true,
            "--graphics-features" => graphics_features = true,
            "--probe-vulkan" => probe_vulkan = true,
            other if other.starts_with('-') => return None,
            other if deploy_dir.is_none() => deploy_dir = Some(PathBuf::from(other)),
            _ => return None,
        }
    }
    Some(Args {
        deploy_dir: deploy_dir?,
        config_path,
        check,
        graphics_features,
        probe_vulkan,
    })
}

fn load_config(config_path: Option<&PathBuf>) -> Result<Option<HostConfig>, ExitCode> {
    match config_path {
        Some(path) => HostConfig::from_path(path).map(Some).map_err(|err| {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            ExitCode::FAILURE
        }),
        None => Ok(None),
    }
}

pub fn run_cli() -> ExitCode {
    let Some(mut args) = parse_args() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if args.graphics_features || args.probe_vulkan {
        if args.check || args.config_path.is_some() || (args.graphics_features && args.probe_vulkan) {
            eprintln!("{USAGE}"); return ExitCode::from(2);
        }
        let platform_selection = match rudroid_common::scan_deploy_dir(&args.deploy_dir) {
            Ok(entries) => {
                let ids: Vec<String> = if entries.iter().any(|e| e.stem == "platform_sdl_runtime") {
                    vec!["platform_sdl_runtime".into()]
                } else {
                    entries
                        .into_iter()
                        .filter(|e| e.stem.starts_with("platform_"))
                        .map(|e| e.stem)
                        .collect()
                };
                Selection::Allowlist(ids)
            }
            Err(error) => {
                rudroid_common::error!("rudroid-unity-loader: {error}");
                return ExitCode::FAILURE;
            }
        };
        let capabilities = match load_cli_kit(&args.deploy_dir, &platform_selection) {
            Ok(value) => value,
            Err(error) => { rudroid_common::error!("rudroid-unity-loader: {error}"); return ExitCode::FAILURE; }
        };
        if args.probe_vulkan {
            return match rudroid_unity_loader::probe_vulkan(&capabilities) {
                Ok(report) => {
                    eprintln!("[VULKAN-PROBE] driver={} version={}.{} guest_frames=0",report.video_driver,report.major,report.minor);
                    println!("RUDROID_VULKAN=1");
                    ExitCode::SUCCESS
                },
                Err(error) => { rudroid_common::error!("[VULKAN-PROBE] {error}"); ExitCode::FAILURE },
            };
        }
        return match rudroid_unity_loader::graphics_feature_tokens(&capabilities) {
            Ok(tokens) => { println!("{tokens}"); ExitCode::SUCCESS },
            Err(error) => { rudroid_common::error!("rudroid-unity-loader: {error}"); ExitCode::FAILURE },
        };
    }
    let config = match load_config(args.config_path.as_ref()) {
        Ok(config) => config,
        Err(code) => return code,
    };

    if let Some(config) = &config {
        if let Err(err) = config.render_dimensions() {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
        if let Err(err) = config.memory_policy() {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
        // Engine-specific HostConfig checks (Unity startup / Mono) run through
        // EngineProfile::validate_config after register-JNI in run mode.
        // --check validates kit topology and generic config only.
    }

    if args.check {
        return check_only(&args.deploy_dir, config.as_ref());
    }

    // Run mode boots a guest: without a Host Config there is nothing to
    // bring up (guest paths, plugin getters and the package gate read it).
    let Some(mut config) = config else {
        rudroid_common::error!(
            "rudroid-unity-loader: run mode requires --config \
             (guest paths and plugin config getters read it); \
             pass --check for descriptor validation only"
        );
        return ExitCode::FAILURE;
    };

    // The C host changes cwd to game_files before native constructors and
    // JNI preparation. Keep deployed-library and game-root paths absolute
    // so relative CLI inputs cannot become gamedata/gamedata after chdir.
    // Missing roots retain the existing ordered phase/guest diagnostics;
    // check mode and explicit phase-only smoke never change cwd.
    let skip_guest = std::env::var("RUDROID_SKIP_GUEST").is_ok_and(|v| v == "1" || v == "true");
    if !skip_guest {
        if let Some(root) = config.paths().game_files.map(PathBuf::from).filter(|p| p.is_dir()) {
            let setup = || -> std::io::Result<(PathBuf, PathBuf)> {
                let cwd = std::env::current_dir()?;
                let deploy = if args.deploy_dir.is_absolute() { args.deploy_dir.clone() } else { cwd.join(&args.deploy_dir) };
                Ok((deploy, root.canonicalize()?))
            };
            let (deploy, root) = match setup() {
                Ok(paths) => paths,
                Err(error) => { rudroid_common::error!("rudroid-unity-loader: game working directory: {error}"); return ExitCode::FAILURE; }
            };
            let Some(value) = root.to_str() else {
                rudroid_common::error!("rudroid-unity-loader: paths.game_files must resolve to a UTF-8 path");
                return ExitCode::FAILURE;
            };
            if !config.set_string("paths.game_files", value) {
                rudroid_common::error!("rudroid-unity-loader: failed to store canonical paths.game_files");
                return ExitCode::FAILURE;
            }
            if let Err(error) = std::env::set_current_dir(&root) {
                rudroid_common::error!("rudroid-unity-loader: cannot enter paths.game_files: {error}");
                return ExitCode::FAILURE;
            }
            args.deploy_dir = deploy;
            rudroid_common::diagnostic!("[PATHS] game working directory: {}", root.display());
        }
    }

    // Install before any plugin phase runs so config getters observe it
    // (ticket 44: the review probe read the fallback because this was never
    // wired).
    if let Err(err) = host_api::install_config(Some(config.clone())) {
        rudroid_common::error!("rudroid-unity-loader: {err}");
        return ExitCode::FAILURE;
    }
    let memory = host_api::memory_policy();
    host_api::loader_log("MEMORY", &format!(
        "mode={} stream_buffer_bytes={} stream_buffer_budget_mib={} preload_audio={}",
        memory.mode.as_str(), memory.stream_buffer_bytes,
        memory.stream_buffer_budget_mib, memory.preload_audio
    ));

    let selection = match Selection::from_config(&config) {
        Ok(selection) => selection,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    let capabilities = match load_cli_kit(&args.deploy_dir, &selection) {
        Ok(capabilities) => capabilities,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    let package = config.package().package_name;
    let capabilities = match bring_up(capabilities, package.as_deref()) {
        Ok(ok) => ok,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    if skip_guest || rudroid_common::logging::diagnostics_enabled() { print_descriptors(&capabilities); }

    if let Some(profile) = runtime::registered_profile() {
        if let Some(validate) = profile.validate_config {
            if validate(Some(host_api::table())) != 0 {
                rudroid_common::error!(
                    "rudroid-unity-loader: engine profile validate_config refused the Host Config"
                );
                return ExitCode::FAILURE;
            }
        }
    }

    // Skip smoke before any guest requirement is evaluated: a config without
    // guest paths still smokes the plugin phases.
    if skip_guest {
        println!(
            "result=skipped guest=skipped capabilities={} \
             (RUDROID_SKIP_GUEST set: plugin/phase smoke only, NOT a game start)",
            capabilities.len()
        );
        return ExitCode::SUCCESS;
    }

    // JNI_OnLoad/bootstrap can start services before initJni. Ordinary early
    // failures must still join those owners before this process returns.
    let mut services = CoreServicesCleanup {
        capabilities: &capabilities,
        armed: true,
        guest_entered: false,
    };
    // The selected platform adapter creates/validates the host session before
    // guest code runs. Native display details stay out of the boot sequence.
    if let Err(err) = rudroid_unity_loader::host_platform::bring_up_host_session(&capabilities, &config) {
        rudroid_common::error!("rudroid-unity-loader: {err}");
        return ExitCode::FAILURE;
    }

    let plan = match GuestPlan::from_config(&config) {
        Ok(plan) => plan,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };

    // The profile lifecycle (ticket 53), in the reference bring-up order
    // (reference main.cpp): prepare constructs the engine session BEFORE any
    // guest module is mapped; bootstrap (`NativeLoader.load`) and run (the
    // render loop) follow the guest bring-up. A profile that never deployed,
    // never registered, or fails any phase exits non-zero naming the stage —
    // the engine is never reported started on a stub.
    if let Err(err) = runtime::prepare_engine_runtime(&capabilities) {
        rudroid_common::error!("rudroid-unity-loader: {err}");
        return ExitCode::FAILURE;
    }
    // Even a partial map/JNI_OnLoad can register process finalizers. After
    // explicit cleanup, failed launches must use the same terminal boundary as
    // successful runs; host atexit must not re-enter retired Unity state.
    let events = match bring_up_cli_guest(&capabilities, &plan, || services.guest_entered = true) {
        Ok(events) => events,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    for event in &events {
        if rudroid_common::logging::diagnostics_enabled() { println!(
            "guest module {:?} mapped (report base=0x{:x})",
            event.kind, event.base
        ); }
    }
    let native_lib_dir = config
        .get_string("paths.library", "lib/arm64-v8a")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "lib/arm64-v8a".to_owned());
    if let Err(err) = runtime::bootstrap_engine_runtime(&capabilities, &native_lib_dir) {
        rudroid_common::error!("rudroid-unity-loader: {err}");
        return ExitCode::FAILURE;
    }
    let completion = match runtime::run_engine_runtime(&capabilities) {
        Ok(completion) => completion,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Full teardown completed, or the explicitly selected process-exit path
    // kept native/service state pinned after the engine finished saving.
    // Neither case may run an additional cleanup cascade before _exit below.
    services.armed = false;
    if completion == runtime::EngineRunCompletion::ProcessExit {
        if rudroid_common::logging::diagnostics_enabled() { println!("result=process-exit saved=true"); }
    }
    if rudroid_common::logging::diagnostics_enabled() { println!("result=started capabilities={}", capabilities.len()); }
    rudroid_unity_loader::terminate_completed_guest(&capabilities, 0)
}

struct CoreServicesCleanup<'a> {
    capabilities: &'a [rudroid_unity_loader::LoadedCapability],
    armed: bool,
    guest_entered: bool,
}
impl Drop for CoreServicesCleanup<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        use rudroid_common::jni_abi::shutdown_runtime_lifecycle;
        use rudroid_common::jni_env::JNI_VERSION_1_6;
        if let Some(env) = rudroid_unity_loader::host_api::table().java_vm()
            .and_then(|vm| vm.get_env(JNI_VERSION_1_6)) {
            // Idempotent after the profile's complete teardown. Pre-init
            // failures never touch Unity nativeDone; only Lifecycle closes.
            let _ = shutdown_runtime_lifecycle(env);
        }
        if self.guest_entered {
            rudroid_unity_loader::terminate_completed_guest(self.capabilities, 1);
        }
    }
}

/// `--check`: load descriptors and validate topology only. Phases and the
/// guest never run here, and the banner says so explicitly.
fn check_only(deploy_dir: &PathBuf, config: Option<&HostConfig>) -> ExitCode {
    let selection = match config.map(Selection::from_config) {
        Some(Ok(selection)) => selection,
        Some(Err(err)) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
        None => Selection::All,
    };
    let capabilities = match load_cli_kit(deploy_dir, &selection) {
        Ok(capabilities) => capabilities,
        Err(err) => {
            rudroid_common::error!("rudroid-unity-loader: {err}");
            return ExitCode::FAILURE;
        }
    };
    let package = config.and_then(|c| c.package().package_name);
    let descriptors: Vec<_> = capabilities.iter().map(|c| c.descriptor.clone()).collect();
    let order =
        match rudroid_unity_loader::validate::validate_and_sort(&descriptors, package.as_deref())
        {
            Ok(order) => order,
            Err(diagnostic) => {
                rudroid_common::error!("rudroid-unity-loader: {}", diagnostic.to_json());
                return ExitCode::FAILURE;
            }
        };
    // The loader pins handles for process life; keep them open.
    std::mem::forget(capabilities);
    print_descriptors_in_order(&order);
    println!(
        "result=check-only capabilities={} guest=not-started (--check validates descriptors only)",
        order.len()
    );
    ExitCode::SUCCESS
}

fn print_descriptors(capabilities: &[rudroid_unity_loader::LoadedCapability]) {
    for capability in capabilities {
        print_descriptor(&capability.descriptor);
    }
}

fn print_descriptors_in_order(descriptors: &[rudroid_unity_loader::DescriptorInfo]) {
    for descriptor in descriptors {
        print_descriptor(descriptor);
    }
}

fn print_descriptor(d: &rudroid_unity_loader::DescriptorInfo) {
    print!("{} kind={} build_id={}", d.id, d.kind, d.build_id);
    if let Some(package) = &d.target_package {
        print!(" target_package={package}");
    }
    if !d.required_plugins.is_empty() {
        print!(" requires={}", d.required_plugins.join(","));
    }
    println!();
}

fn load_cli_kit(
    deploy_dir: &std::path::Path,
    selection: &Selection,
) -> Result<Vec<rudroid_unity_loader::LoadedCapability>, rudroid_unity_loader::LoaderError> {
    // SAFETY: the operator-selected deploy directory is a trusted same-kit
    // native deployment; library handles stay pinned for process life.
    unsafe { load_deploy_dir(deploy_dir, selection) }
}

fn bring_up_cli_guest(
    deployed: &[rudroid_unity_loader::LoadedCapability],
    plan: &GuestPlan,
    entering: impl FnMut(),
) -> Result<Vec<guest::ModuleEvent>, guest::GuestError> {
    // SAFETY: CLI launches the operator-selected trusted Android package;
    // guest mappings and native ABI providers remain process-pinned.
    unsafe { guest::bring_up_observed(deployed, plan, entering) }
}
