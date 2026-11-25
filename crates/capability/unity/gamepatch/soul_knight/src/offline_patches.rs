//! INPUT: verified offline build metadata and instruction guards
//! OUTPUT: checked startup patches and restoration
//! POS: Soul Knight native startup policy

use super::*;

/// Explicit offline policy: defer only the constructor's eager browser warmup.
/// This is not a natural config switch. The original client/settings/Unity
/// object initialization, Core subscriptions and later lazy getter remain real.
/// A later webpage request still reaches the original unsupported Java bridge
/// and may fail; this supplies neither a browser nor SDK/auth success.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub(super) struct WebViewWarmup {
    entry: usize,
    before: Vec<u32>,
    after: [u32; 1],
    getter: [u32; 8],
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
impl WebViewWarmup {
    pub(super) fn prepare(api: &Il2cpp, regions: &MappedRegions) -> Result<Self, &'static str> {
        let ctor = Target {
            namespace: c"ChillyRoom.Webview",
            class: c"WebViewClient",
            method: c".ctor",
            static_: false,
            parameters: &["ChillyRoom.Webview.WebviewSetting"],
            returns: "System.Void",
            rva: 0x99ce59c,
            prefix: "fe0f1cf8f85f01a9f65702a9f44f03a9d54f03b0b80203d077f90290b60203d0",
        };
        let get = Target {
            method: c"GetOrCreateUniWebView",
            parameters: &[],
            returns: "UniWebView",
            rva: 0x99ce6c8,
            prefix: "fe0f1df8f65701a9f44f02a9d44f03b076f9029088467339d67a45f9f30300aa",
            ..ctor
        };
        let ctor_method = target_in_image(api, c"com.chillyroom.service.core.Runtime.dll", ctor)?;
        let getter = target_in_image(api, c"com.chillyroom.service.core.Runtime.dll", get)?;
        let setting = class(
            api,
            c"com.chillyroom.service.core.Runtime.dll",
            c"ChillyRoom.Webview",
            c"WebviewSetting",
        )?;
        if api.method_flags(ctor_method) != Some(6278)
            || api.method_flags(getter) != Some(134)
            || api
                .method_param(ctor_method, 0)
                .and_then(|ty| api.class_from_type(ty))
                != Some(setting)
        {
            return Err("WebView warmup method identity mismatch");
        }
        let entry = api.base() + ctor.rva;
        require_executable_source(regions, entry, 300)
            .map_err(|_| "WebView warmup constructor mapping is not RX")?;
        require_executable_source(regions, entry + 300, 32)
            .map_err(|_| "WebView warmup getter mapping is not RX")?;
        let memory = MappedMemory::open().map_err(|_| "WebView warmup code reader unavailable")?;
        let mut body = [0u8; 300];
        let mut get_bytes = [0u8; 32];
        memory
            .read_exact(entry, &mut body)
            .map_err(|_| "WebView warmup constructor unreadable")?;
        memory
            .read_exact(entry + 300, &mut get_bytes)
            .map_err(|_| "WebView warmup getter unreadable")?;
        let mut hash = sha256::Sha256::new();
        hash.update(&body);
        if sha256::hex(&hash.finish()) != WEBVIEW_CTOR_SHA || sha256::hex(&get_bytes) != get.prefix
        {
            return Err("WebView warmup complete code identity mismatch");
        }
        let words: Vec<_> = body
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().expect("code word")))
            .collect();
        // FDE-backed normal epilogue restores x19..24, SP and LR before its
        // unique B(+8). The following exception exit remains untouched.
        if words[68..]
            != [
                0xaa1303e0, 0xa9434ff4, 0xa94257f6, 0xa9415ff8, 0xf84407fe, 0x14000002, 0x9710f135,
            ]
        {
            return Err("WebView warmup tail identity mismatch");
        }
        Ok(Self {
            entry,
            before: words[..73].to_vec(),
            after: [words[74]],
            getter: std::array::from_fn(|index| {
                u32::from_le_bytes(
                    get_bytes[index * 4..index * 4 + 4]
                        .try_into()
                        .expect("getter word"),
                )
            }),
        })
    }

    /// # Safety
    /// Owned pinned code and the complete original constructor/getter remain
    /// idle and unmodified throughout this startup transaction and rollback.
    pub(super) unsafe fn patch(&self, defer: bool) -> Result<(), arm64_detour::DetourError> {
        let (expected, replacement) = if defer {
            (0x14000002, 0xd65f03c0)
        } else {
            (0xd65f03c0, 0x14000002)
        };
        let guards = [
            arm64_detour::CodeGuard {
                address: self.entry,
                words: &self.before,
            },
            arm64_detour::CodeGuard {
                address: self.entry + 296,
                words: &self.after,
            },
            arm64_detour::CodeGuard {
                address: self.entry + 300,
                words: &self.getter,
            },
        ];
        // SAFETY: caller pins/keeps the entry idle. Guards deliberately exclude
        // the patched word, so the same evidence also permits inverse rollback.
        if !unsafe {
            arm64_detour::patch_words_if_matches(
                self.entry + 292,
                &[expected],
                &[replacement],
                &guards,
            )
        }? {
            return Err(arm64_detour::DetourError::Protection(
                "WebView warmup code guard mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// This explicit offline runtime has no Android notification manager/scheduler.
/// Return the original public API's unsupported result before its prologue,
/// letting guarded callers choose null/void/-1 without setting s_Initialized.
/// This does not implement notification Intent parsing or the whole assembly.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub(super) struct NotificationUnavailable {
    entry: usize,
    saved: [u32; 2],
    remaining: [u32; 6],
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
impl NotificationUnavailable {
    pub(super) fn prepare(api: &Il2cpp, regions: &MappedRegions) -> Result<Self, &'static str> {
        let spec = Target {
            namespace: c"Unity.Notifications.Android",
            class: c"AndroidNotificationCenter",
            method: c"Initialize",
            static_: true,
            parameters: &[],
            returns: "System.Boolean",
            rva: 0xeb64514,
            prefix: "fd0f1cf8fe5f01a9f65702a9f44f03a9ff4307d1b3c300f0d76f00b068325d39",
        };
        let method = target_in_image(api, c"Unity.Notifications.Android.dll", spec)
            .map_err(|_| "notification Initialize method or runtime prefix mismatch")?;
        if api.method_flags(method) != Some(150) {
            return Err("notification Initialize flags mismatch");
        }
        let entry = api.base() + spec.rva;
        require_executable_source(regions, entry, 32)
            .map_err(|_| "notification Initialize source mapping is not RX")?;
        let mut bytes = [0u8; 32];
        MappedMemory::open()
            .map_err(|_| "notification Initialize code reader unavailable")?
            .read_exact(entry, &mut bytes)
            .map_err(|_| "notification Initialize code unreadable")?;
        // The original file is wrapped; only the live, unpacked exact RX
        // prefix is accepted. No guessed decoding or file-byte substitution.
        if sha256::hex(&bytes) != spec.prefix {
            return Err("notification Initialize runtime prefix changed");
        }
        let words: [u32; 8] = std::array::from_fn(|index| {
            u32::from_le_bytes(
                bytes[index * 4..index * 4 + 4]
                    .try_into()
                    .expect("notification word"),
            )
        });
        Ok(Self {
            entry,
            saved: [words[0], words[1]],
            remaining: words[2..].try_into().expect("notification prefix tail"),
        })
    }

    /// # Safety
    /// The exact process-owned source remains pinned and its covered entry
    /// stays idle throughout this startup transaction, including rollback.
    pub(super) unsafe fn patch(&self, unavailable: bool) -> Result<(), arm64_detour::DetourError> {
        let negative = [0x52800000, 0xd65f03c0]; // mov w0, #0; ret, before any stack write
        let (expected, replacement) = if unavailable {
            (&self.saved, &negative)
        } else {
            (&negative, &self.saved)
        };
        let guard = arm64_detour::CodeGuard {
            address: self.entry + 8,
            words: &self.remaining,
        };
        // SAFETY: caller's pinned idle source; compare the full remaining
        // prefix as well as both current entry words before either direction.
        if !unsafe {
            arm64_detour::patch_words_if_matches(self.entry, expected, replacement, &[guard])
        }? {
            return Err(arm64_detour::DetourError::Protection(
                "notification Initialize code guard mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// Optional telemetry stays unavailable in this explicit offline profile.
/// Return from TAUtil.Init before any frame, SDK call, identifier collection,
/// HasInit write or AfterInit callback. This is not analytics success; the
/// original caller continues its other initialization work normally.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub(super) struct AnalyticsUnavailable {
    entry: usize,
    saved: u32,
    remaining: [u32; 7],
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
impl AnalyticsUnavailable {
    pub(super) fn prepare(api: &Il2cpp, regions: &MappedRegions) -> Result<Self, &'static str> {
        let spec = Target {
            namespace: c"",
            class: c"TAUtil",
            method: c"Init",
            static_: true,
            parameters: &[],
            returns: "System.Void",
            rva: 0x652bddc,
            prefix: "ff4302d1fe2300f9fa6705a9f85f06a9f65707a9f44f08a913f404d068fa7a39",
        };
        let method =
            target(api, spec).map_err(|_| "analytics Init method or runtime prefix mismatch")?;
        if api.method_flags(method) != Some(150) {
            return Err("analytics Init flags mismatch");
        }
        let entry = api.base() + spec.rva;
        require_executable_source(regions, entry, 32)
            .map_err(|_| "analytics Init source mapping is not RX")?;
        let mut bytes = [0u8; 32];
        MappedMemory::open()
            .map_err(|_| "analytics Init code reader unavailable")?
            .read_exact(entry, &mut bytes)
            .map_err(|_| "analytics Init code unreadable")?;
        if sha256::hex(&bytes) != spec.prefix {
            return Err("analytics Init runtime prefix changed");
        }
        let words: [u32; 8] = std::array::from_fn(|index| {
            u32::from_le_bytes(
                bytes[index * 4..index * 4 + 4]
                    .try_into()
                    .expect("analytics word"),
            )
        });
        Ok(Self {
            entry,
            saved: words[0],
            remaining: words[1..].try_into().expect("analytics prefix tail"),
        })
    }

    /// # Safety
    /// The exact owned source remains pinned and idle throughout this startup
    /// transaction and its inverse, as with the other source code policies.
    pub(super) unsafe fn patch(&self, unavailable: bool) -> Result<(), arm64_detour::DetourError> {
        let (expected, replacement) = if unavailable {
            (self.saved, 0xd65f03c0)
        } else {
            (0xd65f03c0, self.saved)
        };
        let guard = arm64_detour::CodeGuard {
            address: self.entry + 4,
            words: &self.remaining,
        };
        // SAFETY: caller supplies the pinned idle mapping; check all original
        // remaining prefix words and the current entry before either write.
        if !unsafe {
            arm64_detour::patch_words_if_matches(self.entry, &[expected], &[replacement], &[guard])
        }? {
            return Err(arm64_detour::DetourError::Protection(
                "analytics Init code guard mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/offline_patches.rs"]
mod tests;
