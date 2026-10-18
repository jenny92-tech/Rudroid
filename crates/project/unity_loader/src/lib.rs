//! INPUT:  deploy scanning, same-kit descriptors, libloading and runtime phase contracts
//! OUTPUT:  pinned LoadedCapability, unsafe load_deploy_dir, validation and bring_up
//! POS:    discovers and validates native kit members before running their phases
//! Capability discovery and dynamic loading (ticket 07).
//!
//! The loader owns no capability logic: it scans a Deploy Selection run dir,
//! `dlopen`s every candidate library, reads `rudroid_plugin_descriptor` and
//! refuses members that do not belong to the same build. Library handles stay
//! pinned for the process lifetime — descriptor/phase symbols must remain
//! valid after init.

use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

pub mod guest;
pub mod host_api;
pub mod host_platform;
mod platform_sdl;
pub use platform_sdl::{graphics_feature_tokens, probe_vulkan};
pub mod phases;
pub mod runtime;
mod resources;
mod boot_view;
mod cpuinfo_view;
pub mod select;
pub mod validate;

use std::ffi::CStr;

use rudroid_common::descriptor::PLUGIN_ABI_VERSION;
use rudroid_common::symbols;
use rudroid_common::{check_same_build, scan_deploy_dir, DeployEntry, DeployScanError, Kind};
use libloading::Library;
pub use select::{Selection, SelectionError};
pub use validate::Diagnostic;

use guest::GuestError;
pub use guest::GuestPlan;

/// End the executable after full teardown, or the explicit process-exit
/// completion with saved preferences and all native/service state still pinned.
/// Unity may have retired allocator-owned globals in nativeDone; running
/// its process finalizers again through host atexit can dereference retired data.
/// This is an explicit CLI process boundary, never a library teardown substitute.
/// Training builds first write host LLVM counters for each validated image;
/// a write failure exits nonzero. Failed runs retain their nonzero status and
/// do not contribute training profiles. Production has no collector/export. Flush
/// Rust diagnostics before exit; kernel process exit reclaims pinned mappings.
pub fn terminate_completed_guest(capabilities: &[LoadedCapability], mut status: i32) -> ! {
    use std::io::Write;
    if status == 0 && rudroid_common::pgo::TRAINING {
        for capability in capabilities {
            // SAFETY: these handles are process-pinned, validated same-kit
            // members. Only the training kit exports this no-argument C ABI;
            // it writes that image's LLVM counters and never calls the guest.
            let result = unsafe {
                capability.library().get::<unsafe extern "C" fn() -> i32>(
                    rudroid_common::pgo::FLUSH_SYMBOL.as_bytes(),
                ).map(|flush| flush())
            };
            if !matches!(result, Ok(0)) {
                eprintln!("[PGO] {} profile write failed: {result:?}", capability.descriptor.id);
                status = 1;
            }
        }
        if let Err(error) = rudroid_common::pgo::flush() {
            eprintln!("[PGO] loader profile write failed: {error}");
            status = 1;
        }
        eprintln!("[PGO] collected {} capability images plus loader; status={status}", capabilities.len());
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)] {
        extern "C" { fn _exit(status: i32) -> !; }
        // SAFETY: the POSIX process-exit ABI takes only an int, retains no Rust
        // pointers and never returns. No guest callback or destructor is invoked.
        unsafe { _exit(status) }
    }
    #[cfg(not(unix))]
    std::process::exit(status)
}

/// Metadata read from a capability's C descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorInfo {
    pub id: String,
    pub kind: Kind,
    pub build_id: String,
    pub target_package: Option<String>,
    /// Comma-separated list in the C ABI; split here for validation.
    pub required_plugins: Vec<String>,
}

/// A loaded, pinned capability library plus its descriptor.
///
/// Ownership (ticket 46): the kit is pinned for the process lifetime. Drop
/// intentionally does NOT `dlclose` the library — descriptor and phase
/// symbols must stay valid after init, registered callbacks may live inside
/// the library, and guest code may already have called into it. Dropping a
/// `LoadedCapability` therefore never unloads code; the OS reclaims the
/// mappings at process exit.
pub struct LoadedCapability {
    pub entry: DeployEntry,
    /// Intentionally never `dlclose`d; see the type contract above.
    library: ManuallyDrop<Library>,
    pub descriptor: DescriptorInfo,
}

impl LoadedCapability {
    /// The handle is intentionally never closed; see the type contract.
    pub fn library(&self) -> &Library {
        &self.library
    }
}

impl std::fmt::Debug for LoadedCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedCapability")
            .field("entry", &self.entry)
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum LoaderError {
    Scan(DeployScanError),
    Selection(SelectionError),
    Open { path: PathBuf, reason: String },
    MissingSymbol { path: PathBuf, symbol: &'static str, reason: String },
    /// The descriptor export returned null (ticket 46: structured, no panic).
    NullDescriptor { path: PathBuf },
    /// A descriptor string is not valid UTF-8 (ticket 46: structured).
    InvalidUtf8 { path: PathBuf, reason: String },
    /// The descriptor's `abi_version` is not this loader's (an old or
    /// foreign layout): no later field is read (reference plugin_host order).
    AbiVersionMismatch { path: PathBuf, expected: u32, found: u32 },
    BuildIdMismatch { path: PathBuf, kit: String, found: String },
    /// The descriptor id is empty (reference: `!*descriptor->id` rejects).
    EmptyId { path: PathBuf },
    InvalidKind { path: PathBuf, value: u32 },
    /// The member exports a test-only symbol: it came out of a `cargo test`
    /// feature-unified build (e.g. `test-fixtures`) whose kit id cannot tell
    /// it from a real build (review 2026-09-22 finding 5).
    TestBuildMember { path: PathBuf, symbol: &'static str },
    InitFailed { id: String, code: i32 },
    InitOther { id: String, reason: String },
    PhaseFailed { phase: phases::Phase, id: String, code: i32 },
    PhaseOther { phase: phases::Phase, id: String, reason: String },
    Validation(Diagnostic),
    Guest(GuestError),
    /// The selected platform backend could not create/activate its verified
    /// host session; its own diagnostic and capability id are preserved.
    HostSession { id: String, reason: String },
    /// Run mode without a Host Config: guest paths and plugin getters read it.
    RunConfigRequired,
    /// No Unity profile in the kit: the engine cannot start.
    NoUnityProfile,
    /// The Unity profile lifecycle failed at a named stage (register /
    /// prepare / bootstrap / run, ticket 53): the reason carries the
    /// profile's own diagnostics position.
    ProfileLifecycle {
        id: String,
        stage: &'static str,
        reason: String,
    },
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderError::Scan(e) => write!(f, "{e}"),
            LoaderError::Selection(e) => write!(f, "{e}"),
            LoaderError::Open { path, reason } => {
                write!(f, "cannot load capability {}: {reason}", path.display())
            }
            LoaderError::MissingSymbol { path, symbol, reason } => write!(
                f,
                "capability {} is missing required symbol `{symbol}` ({reason}); \
                 rebuild it with this kit or remove it from the deploy dir",
                path.display()
            ),
            LoaderError::NullDescriptor { path } => write!(
                f,
                "capability {} returned a null descriptor; it is malformed for \
                 this kit — rebuild or remove it",
                path.display()
            ),
            LoaderError::InvalidUtf8 { path, reason } => write!(
                f,
                "descriptor string in {} is not UTF-8 ({reason}); rebuild it \
                 with this kit",
                path.display()
            ),
            LoaderError::AbiVersionMismatch { path, expected, found } => write!(
                f,
                "capability {} has descriptor ABI version {found} but this loader \
                 expects {expected}; rebuild it with this kit",
                path.display()
            ),
            LoaderError::EmptyId { path } => write!(
                f,
                "capability {} has an empty descriptor id; rebuild or remove it",
                path.display()
            ),
            LoaderError::TestBuildMember { path, symbol } => write!(
                f,
                "capability {} exports test-only symbol `{symbol}`: it was built by \
                 `cargo test` with test features; rebuild the kit with \
                 `cargo build --workspace` (scripts/build-kit.sh) before deploying",
                path.display()
            ),
            LoaderError::BuildIdMismatch { path, kit, found } => write!(
                f,
                "capability {} was built for kit `{found}` but this loader is kit `{kit}`; \
                 kits are same-build only — redeploy a consistent set",
                path.display()
            ),
            LoaderError::InvalidKind { path, value } => write!(
                f,
                "capability {} reports unknown kind value {value}",
                path.display()
            ),
            LoaderError::InitFailed { id, code } => write!(
                f,
                "capability `{id}` init failed with code {code}; \
                 see its deploy profile and Host Config, then redeploy"
            ),
            LoaderError::InitOther { id, reason } => {
                write!(f, "capability `{id}` init error: {reason}")
            }
            LoaderError::PhaseFailed { phase, id, code } => write!(
                f,
                "capability `{id}` failed in the {phase:?} phase with code {code}"
            ),
            LoaderError::PhaseOther { phase, id, reason } => {
                write!(f, "capability `{id}` {phase:?} phase error: {reason}")
            }
            LoaderError::Validation(diagnostic) => {
                writeln!(f, "{}", diagnostic.to_json())
            }
            LoaderError::Guest(e) => write!(f, "{e}"),
            LoaderError::HostSession { id, reason } => write!(
                f,
                "host capability `{id}` cannot create its session: {reason}"
            ),
            LoaderError::RunConfigRequired => write!(
                f,
                "run mode requires a Host Config (--config): guest paths and \
                 plugin config getters read it"
            ),
            LoaderError::NoUnityProfile => write!(
                f,
                "deploy dir has no Unity profile capability (unity_2021_3/unity_6); \
                 the engine cannot start"
            ),
            LoaderError::ProfileLifecycle { id, stage, reason } => write!(
                f,
                "unity profile `{id}` failed at the {stage} stage: {reason}"
            ),
        }
    }
}

impl std::error::Error for LoaderError {}

impl From<DeployScanError> for LoaderError {
    fn from(e: DeployScanError) -> Self {
        LoaderError::Scan(e)
    }
}

impl From<SelectionError> for LoaderError {
    fn from(e: SelectionError) -> Self {
        LoaderError::Selection(e)
    }
}

/// Exports that only test-feature builds carry. A `cargo test` run unifies
/// dev-dependency features (`android_elf_load/test-fixtures`) into the
/// shared target's cdylibs without changing the kit id, so the loader — and
/// `scripts/kit_markers.py` for deploy-select/kit-manifest — refuse members
/// exporting any of these. `rudroid_test_build_marker` is the generic
/// marker test features should export. The PGO training collector is likewise
/// non-production; only the generated training loader accepts it after the
/// same-kit check. Keep both lists identical.
pub const TEST_BUILD_MARKERS: [&str; 3] =
    ["rudroid_test_build_marker", "rudroid_elf_image_chain_reset", "rudroid_pgo_training_flush"];

/// Read the C descriptor via `rudroid_plugin_descriptor`.
///
/// # Safety
/// `library` must be a valid handle whose descriptor symbol, if present,
/// follows the kit C ABI.
unsafe fn read_descriptor(library: &Library, path: &Path) -> Result<DescriptorInfo, LoaderError> {
    let symbol: libloading::Symbol<
        unsafe extern "C" fn() -> *const rudroid_common::descriptor::CRudroidPluginDescriptor,
    > = library
        .get(symbols::DESCRIPTOR.as_bytes())
        .map_err(|reason| LoaderError::MissingSymbol {
            path: path.to_path_buf(),
            symbol: symbols::DESCRIPTOR,
            reason: reason.to_string(),
        })?;
    let raw = symbol();
    if raw.is_null() {
        // Structured diagnostics, never panics (ticket 46): a null
        // descriptor is a load error reported per path.
        return Err(LoaderError::NullDescriptor {
            path: path.to_path_buf(),
        });
    }
    let c = unsafe { &*raw };

    // The version is the only field every layout shares: check it before
    // following any pointer of a possibly foreign descriptor.
    if c.abi_version != PLUGIN_ABI_VERSION {
        return Err(LoaderError::AbiVersionMismatch {
            path: path.to_path_buf(),
            expected: PLUGIN_ABI_VERSION,
            found: c.abi_version,
        });
    }

    let build_id = c_str(c.runtime_build_id, path, symbols::DESCRIPTOR)?;
    check_same_build(&build_id).map_err(|(kit, found)| LoaderError::BuildIdMismatch {
        path: path.to_path_buf(),
        kit: kit.to_owned(),
        found: found.to_owned(),
    })?;

    let id = c_str(c.id, path, symbols::DESCRIPTOR)?;
    if id.is_empty() {
        return Err(LoaderError::EmptyId {
            path: path.to_path_buf(),
        });
    }

    let kind_value = c.kind;
    let kind = Kind::from_u32(kind_value).ok_or(LoaderError::InvalidKind {
        path: path.to_path_buf(),
        value: kind_value,
    })?;

    for symbol in TEST_BUILD_MARKERS {
        if rudroid_common::pgo::TRAINING && symbol == rudroid_common::pgo::FLUSH_SYMBOL {
            continue;
        }
        if library.get::<*const std::ffi::c_void>(symbol.as_bytes()).is_ok() {
            return Err(LoaderError::TestBuildMember {
                path: path.to_path_buf(),
                symbol,
            });
        }
    }

    Ok(DescriptorInfo {
        id,
        kind,
        build_id,
        target_package: opt_c_str(c.target_package, path)?,
        required_plugins: opt_c_str(c.required_plugins, path)?
            .map(|joined| {
                // An empty manifest means "no dependencies"; empty items
                // inside a non-empty manifest are a descriptor bug and must
                // reach `validate` as `invalid_dependency`, not vanish here.
                if joined.is_empty() {
                    Vec::new()
                } else {
                    joined.split(',').map(str::to_owned).collect()
                }
            })
            .unwrap_or_default(),
    })
}

/// # Safety
/// `ptr` must be NUL-terminated and valid (non-null).
unsafe fn c_str(
    ptr: *const std::os::raw::c_char,
    path: &Path,
    _symbol: &'static str,
) -> Result<String, LoaderError> {
    if ptr.is_null() {
        return Err(LoaderError::NullDescriptor {
            path: path.to_path_buf(),
        });
    }
    // SAFETY: caller guarantees a valid, NUL-terminated string.
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|reason| LoaderError::InvalidUtf8 {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        })?
        .to_owned();
    Ok(text)
}

unsafe fn opt_c_str(
    ptr: *const std::os::raw::c_char,
    path: &Path,
) -> Result<Option<String>, LoaderError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: caller guarantees a valid, NUL-terminated string.
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|reason| LoaderError::InvalidUtf8 {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        })?
        .to_owned();
    Ok(Some(text))
}

/// Validate topology, sort into reference load order and run all three
/// phases (early init -> register JNI -> init). This is the full ticket 31
/// bring-up path the CLI uses. A failure shuts the runtime registry down so
/// no registered callback can fire after the failed init (ticket 45).
pub fn bring_up(
    mut capabilities: Vec<LoadedCapability>,
    config_package: Option<&str>,
) -> Result<Vec<LoadedCapability>, LoaderError> {
    let descriptors: Vec<DescriptorInfo> = capabilities
        .iter()
        .map(|c| c.descriptor.clone())
        .collect();
    let order = match validate::validate_and_sort(&descriptors, config_package) {
        Ok(order) => order,
        Err(diagnostic) => {
            runtime::shutdown();
            return Err(LoaderError::Validation(diagnostic));
        }
    };
    // Reorder the loaded capabilities to match the validated order. Handles
    // stay open for the process lifetime regardless of later failures.
    let ranks: std::collections::HashMap<_, _> = order.iter()
        .enumerate().map(|(rank, d)| (d.id.as_str(), rank)).collect();
    capabilities.sort_unstable_by_key(|c| ranks[c.descriptor.id.as_str()]);
    if let Err(err) = phases::run_all(&capabilities) {
        runtime::shutdown();
        return Err(err);
    }
    Ok(capabilities)
}

/// The profile call point of the run flow is the ticket-53 runner in
/// [`runtime`]: `prepare_unity_runtime` (before the guest modules are
/// mapped, reference order), `bootstrap_unity_runtime` (after them) and
/// `run_unity_runtime` (the engine loop with the smoke frame limit). With
/// no Unity profile in the kit `prepare_unity_runtime` reports
/// [`LoaderError::NoUnityProfile`].

/// Load (and pin) every capability library selected from `deploy_dir`.
/// The selection (`[plugins].enabled` allowlist or load-all) filters scanned
/// files before opening; descriptor validation and phase execution happen
/// later (ticket 31).
///
/// # Safety
/// Selected files are trusted native kit members. Constructors run during
/// opening, before descriptor validation; exports, descriptor pointers and
/// phase callbacks must obey the same-kit ABI and Host API contracts.
/// Files must not be changed during loading. Opened libraries stay pinned,
/// including members rejected after their constructors have run.
pub unsafe fn load_deploy_dir(
    deploy_dir: &Path,
    selection: &Selection,
) -> Result<Vec<LoadedCapability>, LoaderError> {
    let entries = selection.apply(scan_deploy_dir(deploy_dir)?)?;
    let mut loaded = Vec::with_capacity(entries.len());
    for entry in entries {
        unsafe {
            // SAFETY: the caller authorizes trusted native constructors and
            // the same-kit export contract. Pin before reading descriptors:
            // a constructor can already have published function pointers.
            let library = ManuallyDrop::new(Library::new(&entry.path).map_err(|reason| LoaderError::Open {
                path: entry.path.clone(),
                reason: reason.to_string(),
            })?);
            let descriptor = read_descriptor(&library, &entry.path)?;
            loaded.push(LoadedCapability {
                entry,
                library,
                descriptor,
            });
        }
    }
    Ok(loaded)
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
