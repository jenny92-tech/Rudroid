//! INPUT:  explicit GPU worker name/policy and initially allowed CPU topology
//! OUTPUT:  one current-worker placement after the first successful guest present
//! POS:    shares GLES/Vulkan startup scheduling without polling or another worker
#![forbid(unsafe_code)]
use rudroid_common::{host_api::HostApiV0, thread_placement::ThreadPlacement};
use std::sync::{Once, OnceLock};

static POLICY: OnceLock<Option<(String, ThreadPlacement)>> = OnceLock::new();
static FIRST: Once = Once::new();

pub(super) fn configure(api: &HostApiV0) {
    POLICY.get_or_init(|| {
        if !api.config_bool(c"gpu.separateRenderThread", false) { return None; }
        let name = api.config_string(c"gpu.renderThreadName", 17).unwrap_or_default();
        if name.is_empty() || name.len() > 15 || !name.bytes().all(|b| b.is_ascii_graphic()) {
            eprintln!("[SDL] render thread placement skipped: invalid worker name");
            return None;
        }
        match ThreadPlacement::capture() {
            Ok(Some(plan)) => Some((name, plan)),
            Ok(None) => { eprintln!("[SDL] render thread placement skipped: topology is outside validated shape"); None }
            Err(error) => { eprintln!("[SDL] render thread placement unavailable: {error}"); None }
        }
    });
}

/// Existing first-present counters call this exactly once per guest session.
/// No frame-time filesystem lookup, timer, cross-thread handle or policy loop.
pub(super) fn first_present() {
    let Some(Some((name, plan))) = POLICY.get() else { return; };
    FIRST.call_once(|| match plan.apply_current(name) {
        Ok(true) => eprintln!("[SDL] render thread separated: name={name} from=[{}] to={:?}",
            plan.primary_cpu(), plan.worker_cpus()),
        Ok(false) => eprintln!("[SDL] render thread placement skipped: presenting thread or affinity differs"),
        Err(error) => eprintln!("[SDL] render thread placement failed; engine placement retained: {error}"),
    });
}
