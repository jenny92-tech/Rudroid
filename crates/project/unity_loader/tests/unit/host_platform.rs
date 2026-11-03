//! INPUT:  backend-neutral startup transaction with an owned synthetic display session
//! OUTPUT:  failure/unwind ownership and measured-output/render separation regressions
//! POS:    verifies platform orchestration independently of SDL, guest code and Unity versions
use super::*;
use std::{cell::RefCell, rc::Rc};

type Events = Rc<RefCell<Vec<&'static str>>>;
struct Backend {
    events: Events,
    report: HostSessionReport,
    fail: Option<&'static str>,
}
struct Session {
    events: Events,
    report: HostSessionReport,
    fail: Option<&'static str>,
    pinned: bool,
}
impl DisplayBackend for Backend {
    type Session = Session;
    fn id(&self) -> &str {
        "test_display_without_sdl"
    }
    fn create(&self, _config: &HostConfig) -> Result<Session, String> {
        self.events.borrow_mut().push("create");
        if self.fail == Some("create") {
            return Err("create refused".into());
        }
        Ok(Session {
            events: self.events.clone(),
            report: self.report.clone(),
            fail: self.fail,
            pinned: false,
        })
    }
}
impl DisplaySession for Session {
    fn report(&self) -> Result<HostSessionReport, String> {
        self.events.borrow_mut().push("report");
        if self.fail == Some("report") {
            return Err("report refused".into());
        }
        Ok(self.report.clone())
    }
    fn activate(&mut self, geometry: RenderDimensions) -> Result<(), String> {
        assert_eq!((geometry.output_width, geometry.output_height), (960, 720));
        self.events.borrow_mut().push("activate");
        if self.fail == Some("activate") {
            return Err("activate refused".into());
        }
        Ok(())
    }
    fn pin(mut self) {
        self.events.borrow_mut().push("pin");
        self.pinned = true;
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.events
            .borrow_mut()
            .push(if self.pinned { "retained" } else { "destroy" });
    }
}
fn backend() -> Backend {
    Backend {
        events: Rc::new(RefCell::new(Vec::new())),
        fail: None,
        report: HostSessionReport {
            graphics_api: rudroid_host_config::GraphicsApi::Gles,
            window_w: 960,
            window_h: 720,
            video_driver: "test-native".into(),
            major: 3,
            minor: 0,
            profile_mask: 4,
            swap_count: 1,
            readback_non_background: true,
            gl_vendor: "fixture".into(),
            gl_renderer: "fixture".into(),
            gl_version: "OpenGL ES 3.0".into(),
        },
    }
}
fn config(extra: &str) -> HostConfig {
    HostConfig::parse(&format!(
        "[device]\ndisplayWidth=640\ndisplayHeight=480\n{extra}"
    ))
    .unwrap()
}
fn error(error: LoaderError) -> String {
    match error {
        LoaderError::HostSession { id, reason } => {
            assert_eq!(id, "test_display_without_sdl");
            reason
        }
        other => panic!("wrong error: {other:?}"),
    }
}

#[test]
fn measured_output_and_requested_render_stay_distinct_before_activation() {
    for (extra, expected) in [
        (
            "[gpu]\nrenderWidth=640\nrenderHeight=480\nrenderScalePercent=75",
            (640, 480),
        ),
        ("[gpu]\nrenderScalePercent=75", (720, 540)),
    ] {
        let b = backend();
        let report = start_backend(&b, &config(extra), |geometry| {
            b.events.borrow_mut().push("publish");
            assert_eq!((geometry.output_width, geometry.output_height), (960, 720));
            assert_eq!((geometry.width, geometry.height), expected);
            Ok(())
        })
        .unwrap();
        assert_eq!((report.window_w, report.window_h), (960, 720));
        assert_eq!(
            *b.events.borrow(),
            ["create", "report", "publish", "activate", "pin", "retained"]
        );
    }
}

#[test]
fn creation_and_report_failures_keep_backend_identity_and_release_once() {
    for stage in ["create", "report"] {
        let mut b = backend();
        b.fail = Some(stage);
        assert!(
            error(start_backend(&b, &config(""), |_| panic!("must not publish")).unwrap_err())
                .contains(stage)
        );
        let expected = if stage == "create" {
            vec!["create"]
        } else {
            vec!["create", "report", "destroy"]
        };
        assert_eq!(*b.events.borrow(), expected);
    }
}

#[test]
fn unverified_or_invalid_output_never_publishes_or_activates() {
    for condition in ["profile", "swap", "readback", "size", "render-policy"] {
        let mut b = backend();
        match condition {
            "profile" => b.report.profile_mask = 1,
            "swap" => b.report.swap_count = 0,
            "readback" => b.report.readback_non_background = false,
            "size" => b.report.window_w = 0,
            _ => {}
        }
        let c = config(if condition == "render-policy" {
            "[gpu]\nrenderScalePercent=25"
        } else {
            ""
        });
        error(start_backend(&b, &c, |_| panic!("must not publish")).unwrap_err());
        assert_eq!(*b.events.borrow(), ["create", "report", "destroy"]);
    }
}

#[test]
fn publication_and_activation_failure_destroy_unpinned_session_once() {
    for stage in ["publish", "activate"] {
        let mut b = backend();
        b.fail = Some(stage);
        assert!(error(
            start_backend(&b, &config(""), |_| {
                b.events.borrow_mut().push("publish");
                if stage == "publish" {
                    Err("publish refused")
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
        )
        .contains(stage));
        let expected = if stage == "publish" {
            vec!["create", "report", "publish", "destroy"]
        } else {
            vec!["create", "report", "publish", "activate", "destroy"]
        };
        assert_eq!(*b.events.borrow(), expected);
    }
}

#[test]
fn unwind_during_publication_releases_session_without_activation() {
    let b = backend();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = start_backend(&b, &config(""), |_| {
            b.events.borrow_mut().push("publish");
            panic!("publication consumer failed");
        });
    }));
    assert!(result.is_err());
    assert_eq!(
        *b.events.borrow(),
        ["create", "report", "publish", "destroy"]
    );
}

#[test]
fn absent_backend_explicitly_stays_headless() {
    assert!(bring_up_host_session(&[], &config("")).unwrap().is_none());
}

#[test]
fn vulkan_preflight_has_an_explicit_contract_without_fabricated_frame_evidence() {
    let mut b = backend();
    b.report.graphics_api = GraphicsApi::Vulkan;
    b.report.major = 1;
    b.report.minor = 2;
    b.report.profile_mask = 0;
    b.report.swap_count = 0;
    b.report.readback_non_background = false;
    let c = config("[gpu]\ngraphicsApi='vulkan'");
    let report = start_backend(&b, &c, |_|Ok(())).unwrap();
    assert_eq!(report.swap_count, 0);
    assert!(!report.readback_non_background);
    b.report.swap_count = 1;
    assert!(error(start_backend(&b, &c, |_|panic!("false frame proof must not publish")).unwrap_err()).contains("preflight"));
}

#[test]
fn graphics_api_selection_is_explicit_and_never_silently_falls_back() {
    let b = backend();
    for extra in ["[gpu]\ngraphicsApi='vulkan'", "[gpu]\ngraphicsApi='invalid'", "[gpu]\ngraphicsApi=7"] {
        assert!(start_backend(&b, &config(extra), |_|panic!("wrong API must not publish")).is_err());
    }
}
