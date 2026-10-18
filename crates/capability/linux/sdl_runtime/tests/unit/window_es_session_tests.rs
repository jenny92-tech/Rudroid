//! INPUT:  private items of `src/window.rs` via `super`
//! OUTPUT:  `es_session_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/window.rs`; declared there with `#[path]`
//! The real ES session needs a display (Xvfb in Docker). With the
//! `sdl` feature the test always runs: no display is a failure,
//! never a silent pass. Builds without the feature carry an ignored
//! placeholder (`window_config_tests`) so the skip shows in the
//! summary.

use super::*;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    None,
    ProgramCreate,
    VertexCreate,
    FragmentCreate,
    VertexCompile,
    FragmentCompile,
    Link,
    Background,
    Foreground,
    BufferCreate,
    TextureCreate,
    TextureUpload,
    Attribute,
    GlError,
}

struct Probe {
    original: GlFns,
    failure: Failure,
    shaders: Vec<u32>,
    programs: Vec<u32>,
    buffers: Vec<u32>,
    textures: Vec<u32>,
    attribute: Option<u32>,
    reads: usize,
    foreground: Option<[u8; 4]>,
    size: (u32, u32),
    frame: Vec<u8>,
}

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

fn probe<R>(run: impl FnOnce(&mut Probe) -> R) -> R {
    PROBE.with(|slot| run(slot.borrow_mut().as_mut().expect("active GL probe")))
}

unsafe extern "C" fn create_shader(kind: u32) -> u32 {
    probe(|p| {
        if (kind == gl::VERTEX_SHADER && p.failure == Failure::VertexCreate)
            || (kind == gl::FRAGMENT_SHADER && p.failure == Failure::FragmentCreate)
        {
            return 0;
        }
        let shader = unsafe { (p.original.create_shader)(kind) };
        p.shaders.push(shader);
        shader
    })
}

unsafe extern "C" fn create_program() -> u32 {
    probe(|p| {
        if p.failure == Failure::ProgramCreate {
            return 0;
        }
        let program = unsafe { (p.original.create_program)() };
        p.programs.push(program);
        program
    })
}

unsafe extern "C" fn gen_buffers(count: i32, out: *mut u32) {
    probe(|p| {
        assert_eq!(count, 1);
        if p.failure == Failure::BufferCreate {
            unsafe { *out = 0 };
            return;
        }
        unsafe { (p.original.gen_buffers)(count, out) };
        p.buffers.push(unsafe { *out });
    });
}

unsafe extern "C" fn gen_textures(count: i32, out: *mut u32) {
    probe(|p| unsafe {
        assert_eq!(count, 1);
        if p.failure == Failure::TextureCreate { *out = 0; return; }
        (p.original.gen_textures)(count, out);
        p.textures.push(*out);
    });
}

unsafe extern "C" fn tex_image_2d(
    target: u32, level: i32, internal: i32, width: i32, height: i32, border: i32,
    format: u32, kind: u32, data: *const c_void,
) {
    probe(|p| unsafe {
        (p.original.tex_image_2d)(target, level, internal,
            if p.failure == Failure::TextureUpload { -1 } else { width },
            height, border, format, kind, data);
    });
}

unsafe extern "C" fn shader_status(shader: u32, name: u32, out: *mut i32) {
    probe(|p| unsafe {
        (p.original.get_shader_iv)(shader, name, out);
        if (p.failure == Failure::VertexCompile && p.shaders.len() == 1)
            || (p.failure == Failure::FragmentCompile && p.shaders.len() == 2)
        {
            *out = 0;
        }
    });
}

unsafe extern "C" fn program_status(program: u32, name: u32, out: *mut i32) {
    probe(|p| unsafe {
        (p.original.get_program_iv)(program, name, out);
        if p.failure == Failure::Link {
            *out = 0;
        }
    });
}

unsafe extern "C" fn attribute(program: u32, name: *const u8) -> i32 {
    probe(|p| {
        if p.failure == Failure::Attribute {
            -1
        } else {
            let location = unsafe { (p.original.get_attrib_location)(program, name) };
            if location >= 0 {
                p.attribute = Some(location as u32);
            }
            location
        }
    })
}

unsafe extern "C" fn read_pixels(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    format: u32,
    kind: u32,
    out: *mut c_void,
) {
    probe(|p| unsafe {
        (p.original.read_pixels)(x, y, w, h, format, kind, out);
        p.reads += 1;
        if p.reads == 2 {
            assert_eq!((w, h, format, kind), (1, 1, gl::RGBA, gl::UNSIGNED_BYTE));
            p.foreground = Some(*out.cast::<[u8; 4]>());
            if p.failure == Failure::None {
                p.frame.resize((p.size.0 * p.size.1 * 4) as usize, 0);
                (p.original.read_pixels)(0, 0, p.size.0 as i32, p.size.1 as i32,
                    gl::RGBA, gl::UNSIGNED_BYTE, p.frame.as_mut_ptr().cast());
            }
        }
        if p.failure == Failure::Background {
            *out.cast::<u8>() ^= 0xff;
        }
        if p.failure == Failure::Foreground && p.reads == 2 {
            *out.cast::<u8>() = 0;
        }
    });
}

unsafe extern "C" fn get_error() -> u32 {
    probe(|p| {
        let error = unsafe { (p.original.get_error)() };
        if p.failure == Failure::GlError {
            0x0502
        } else {
            error
        }
    })
}

fn verification_objects_are_reclaimed_on_success_and_every_early_error(
    session: &SdlSession,
    negative_tests: bool,
) {
    // Reuse the display test's current context on its SDL owner thread;
    // separate Rust tests would initialize SDL on different threads.
    let video = session._sdl.video().unwrap();
    let original = unsafe { load_gl_fns(&video).unwrap() };
    // These queries run against the real current driver, not the probe.
    let object_query = |name| {
        let pointer = video.gl_get_proc_address(name);
        assert!(!pointer.is_null(), "missing GPU lifetime query {name}");
        unsafe {
            std::mem::transmute::<*const (), unsafe extern "C" fn(u32) -> u8>(pointer)
        }
    };
    let is_shader = object_query("glIsShader");
    let is_program = object_query("glIsProgram");
    let is_buffer = object_query("glIsBuffer");
    let is_texture = object_query("glIsTexture");
    let pointer = video.gl_get_proc_address("glGetVertexAttribiv");
    assert!(!pointer.is_null());
    let get_attribute = unsafe {
        std::mem::transmute::<*const (), unsafe extern "C" fn(u32, u32, *mut i32)>(pointer)
    };
    let granted = (session.report.major as u8, session.report.minor as u8);
    let mut cases = vec![(Failure::None, granted), (Failure::None, (2, 0))];
    if negative_tests { cases.extend([
        Failure::ProgramCreate,
        Failure::VertexCreate,
        Failure::FragmentCreate,
        Failure::VertexCompile,
        Failure::FragmentCompile,
        Failure::Link,
        Failure::Background,
        Failure::Foreground,
        Failure::BufferCreate,
        Failure::TextureCreate,
        Failure::TextureUpload,
        Failure::Attribute,
        Failure::GlError,
    ].into_iter().map(|failure| (failure, granted))); }
    for (failure, dialect) in cases {
        PROBE.with(|slot| {
            *slot.borrow_mut() = Some(Probe {
                original,
                failure,
                shaders: Vec::new(),
                programs: Vec::new(),
                buffers: Vec::new(),
                textures: Vec::new(),
                attribute: None,
                reads: 0,
                foreground: None,
                size: session._window.drawable_size(),
                frame: Vec::new(),
            })
        });
        let fns = GlFns {
            create_shader,
            create_program,
            gen_buffers,
            gen_textures,
            tex_image_2d,
            get_shader_iv: shader_status,
            get_program_iv: program_status,
            get_attrib_location: attribute,
            read_pixels,
            get_error,
            ..original
        };
        let swaps = AtomicU32::new(0);
        let result = unsafe {
            draw_and_verify_frame(
                &fns,
                &session._window,
                dialect,
                &swaps,
            )
        };
        let p = PROBE.with(|slot| slot.borrow_mut().take().unwrap());
        if failure == Failure::None {
            assert!(matches!(result, Ok(true)), "{result:?}");
            let foreground = p.foreground.expect("real visible-frame readback");
            assert!(foreground[..3].iter().all(|channel| *channel <= 32),
                "startup must show a neutral dark frame, never diagnostic red: {foreground:?}");
            assert_eq!(foreground[3], 255);
            let purple = p.frame.chunks_exact(4).filter(|px| px[0] > 60 && px[2] > 100
                && u16::from(px[0]) > u16::from(px[1]) + 20
                && u16::from(px[2]) > u16::from(px[1]) + 30).count();
            assert!(purple > 10, "the real GLES{dialect:?} frame must contain the purple mascot");
            let white = p.frame.chunks_exact(4).take((p.size.0 * p.size.1 / 2) as usize)
                .filter(|px| px[..3].iter().all(|v| *v > 210)).count();
            assert!(white > 5, "the lower half must contain the wordmark");
            assert!(p.frame.chunks_exact(4).all(|px| px[3] == 255));
            if p.size.0 >= 640 {
                let dir = std::env::var_os("CARGO_TARGET_DIR").expect("fixed Docker target");
                let file = std::path::PathBuf::from(dir).join("startup-brand.ppm");
                let mut ppm = format!("P6\n{} {}\n255\n", p.size.0, p.size.1).into_bytes();
                for row in p.frame.chunks_exact(p.size.0 as usize * 4).rev() {
                    for px in row.chunks_exact(4) { ppm.extend_from_slice(&px[..3]); }
                }
                std::fs::write(&file, ppm).unwrap();
                eprintln!("[STARTUP BRAND] GLES{dialect:?} capture={}", file.display());
            }
            assert_eq!(
                (p.shaders.len(), p.programs.len(), p.buffers.len()),
                (2, 1, 1)
            );
            assert_eq!(p.textures.len(), 1);
            assert_eq!(swaps.load(Ordering::Relaxed), 1);
        } else if failure == Failure::Foreground {
            assert!(
                matches!(result, Ok(false)),
                "must preserve failed pixel evidence"
            );
            assert_eq!(swaps.load(Ordering::Relaxed), 1);
        } else {
            assert!(result.is_err(), "{failure:?} must fail explicitly");
            assert_eq!(swaps.load(Ordering::Relaxed), 0);
        }
        for shader in p.shaders {
            assert_eq!(
                unsafe { is_shader(shader) },
                0,
                "{failure:?} retained shader {shader}"
            );
        }
        for program in p.programs {
            assert_eq!(
                unsafe { is_program(program) },
                0,
                "{failure:?} retained program {program}"
            );
        }
        for buffer in p.buffers {
            assert_eq!(
                unsafe { is_buffer(buffer) },
                0,
                "{failure:?} retained VBO {buffer}"
            );
        }
        for texture in p.textures {
            assert_eq!(unsafe { is_texture(texture) }, 0,
                "{failure:?} retained startup texture {texture}");
        }
        if let Some(location) = p.attribute {
            for name in [0x889F, 0x8622] {
                // buffer binding and enabled flag
                let mut value = -1;
                unsafe { get_attribute(location, name, &mut value) };
                assert_eq!(value, 0, "{failure:?} retained vertex attribute {name:#x}");
            }
        }
        assert_eq!(
            unsafe { (original.get_error)() },
            0,
            "cleanup must be valid GLES"
        );
        eprintln!("[GPU LIFETIME] {failure:?}: retained objects=0");
    }
}

#[test]
fn requested_es_context_is_actually_granted_es_and_drawing_verifies() {
    assert!(
        std::env::var("RUDROID55_DISPLAY").is_ok_and(|v| v == "1"),
        "the ES session test needs a display: run it through \
                         `bash crates/scripts/docker-rust.sh test --sdl` (xvfb-run + Mesa \
                         software GLES, RUDROID55_DISPLAY=1)"
    );
    let (mut session, report) =
        create_session_sized(64, 64, 3, 2).expect("real ES session under Xvfb");
    assert_eq!(
        report.profile_mask,
        gl::PROFILE_ES as u32,
        "profile must be ES"
    );
    assert!(
        report.gl_version.to_bytes().starts_with(b"OpenGL ES"),
        "version string must identify ES: {}",
        report.gl_version.to_string_lossy()
    );
    assert!(
        !report.gl_vendor.to_bytes().is_empty(),
        "GL_VENDOR must identify the implementation"
    );
    assert_ne!(
        report.gl_vendor, report.gl_version,
        "vendor must not be read from GL_VERSION"
    );
    assert_ne!(
        report.video_driver.to_str().unwrap(),
        "dummy",
        "dummy video is not a display"
    );
    assert!(report.swap_count >= 1, "at least one real swap");
    assert!(
        report.readback_non_background,
        "the drawn triangle must survive the pixel readback"
    );
    eprintln!(
        "[EVIDENCE] driver={} renderer={:?} vendor={:?} version={:?} gl_strings={:?}",
        report.video_driver.to_string_lossy(),
        report.gl_renderer,
        report.gl_vendor,
        report.gl_version,
        report.gl_strings
    );
    verification_objects_are_reclaimed_on_success_and_every_early_error(&session, true);
    #[cfg(target_os = "linux")]
    framebuffer_hint_policy_preserves_real_context_state(&session);
    assert!(Arc::ptr_eq(&report.gl_version, &report.gl_strings[0]));
    assert!(Arc::ptr_eq(&report.gl_renderer, &report.gl_strings[1]));
    assert!(Arc::ptr_eq(&report.gl_vendor, &report.gl_strings[2]));
    assert!(Arc::ptr_eq(
        &report.gl_version,
        &session.report().gl_version
    ));
    #[cfg(target_os = "linux")]
    let graphics = super::super::guest_graphics::tests::exercise(&mut session);
    let strings = [
        Arc::downgrade(&report.video_driver),
        Arc::downgrade(&report.gl_version),
        Arc::downgrade(&report.gl_renderer),
        Arc::downgrade(&report.gl_vendor),
    ];
    drop(session);
    #[cfg(target_os = "linux")]
    assert!(
        graphics.upgrade().is_none(),
        "TLS cache must not retain the closed graphics service"
    );
    assert!(
        strings.iter().all(|string| string.upgrade().is_some()),
        "owned Rust snapshots survive context destruction"
    );
    assert!(report.gl_version.to_bytes().starts_with(b"OpenGL ES"));
    #[cfg(target_os = "linux")]
    let is_wayland = report.video_driver.to_bytes() == b"wayland";
    drop(report);
    assert!(
        strings.iter().all(|string| string.upgrade().is_none()),
        "last report/session release must reclaim all cached strings"
    );
    #[cfg(target_os = "linux")]
    {
        // Same owning test thread: a rejected backend must leave no SDL window/context,
        // and the original path must still initialize immediately afterwards.
        if !is_wayland {
            let rejected = create_session_native_buffer(64, 64, 3, 2);
            assert!(matches!(rejected, Err(SdlError::Window(ref message)) if message.contains("requires Wayland")));
        }
        {
            let sdl = sdl2::init().unwrap(); let video = sdl.video().unwrap();
            // No GL context exists: getters would panic here on the real SDL2 API.
            let format = NativeBackbufferFormat::select(&video);
            drop(format); // Request reset must not need a current context either.
        }
        let (mut restarted, _) = create_session_sized(64, 64, 3, 2).unwrap();
        let graphics =
            super::super::guest_graphics::verify_restarted_session(&mut restarted);
        drop(restarted);
        assert!(graphics.upgrade().is_none());
        super::super::guest_graphics::tests::exercise_render_scaling();
    }
    let (brand_session, _) = create_session_sized(640, 480, 3, 2).unwrap();
    verification_objects_are_reclaimed_on_success_and_every_early_error(&brand_session, false);
}

#[cfg(target_os = "linux")]
fn framebuffer_hint_policy_preserves_real_context_state(session: &SdlSession) {
    use super::super::invalidate_policy;
    let video = session._sdl.video().unwrap();
    // SAFETY: this test owns the live current GLES context and exact GL prototypes.
    unsafe {
        let gl = load_gl_fns(&video).unwrap();
        assert_eq!((gl.get_error)(), 0);
        let mut before = [0u8; 4];
        (gl.read_pixels)(32, 32, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, before.as_mut_ptr().cast());
        for name in [b"glInvalidateFramebuffer".as_slice(), b"glDiscardFramebufferEXT"] {
            let hint: extern "C" fn(u32, i32, *const u32) = std::mem::transmute(invalidate_policy::symbol(true, name));
            hint(u32::MAX, i32::MAX, usize::MAX as *const u32);
            assert_eq!((gl.get_error)(), 0);
        }
        let hint: extern "C" fn(u32, i32, *const u32, i32, i32, i32, i32) =
            std::mem::transmute(invalidate_policy::symbol(true, b"glInvalidateSubFramebuffer"));
        hint(u32::MAX, i32::MAX, usize::MAX as *const u32, -1, -1, -1, -1);
        let mut after = [0u8; 4];
        (gl.read_pixels)(32, 32, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, after.as_mut_ptr().cast());
        assert_eq!(before, after);
        assert_eq!((gl.get_error)(), 0);
        let native = video.gl_get_proc_address("glInvalidateFramebuffer");
        assert!(!native.is_null());
        let invalidate: unsafe extern "C" fn(u32, i32, *const u32) = std::mem::transmute(native);
        invalidate(u32::MAX, 0, std::ptr::null());
        assert_eq!((gl.get_error)(), 0x0500, "native fallback still validates GL targets");
        assert_eq!((gl.get_error)(), 0);
    }
}
