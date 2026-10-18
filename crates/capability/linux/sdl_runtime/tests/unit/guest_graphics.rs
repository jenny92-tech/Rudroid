//! INPUT:  private items of `src/guest_graphics.rs` via `super`
//! OUTPUT:  native query, presentation-state and graphics lifecycle regressions under cfg(test)
//! POS:    keeps test bodies out of `src/guest_graphics.rs`; declared there with `#[path]`
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};

#[test]
fn unity_egl_imports_are_linkable_without_creating_a_gles_session() {
    // Actual strong EGL imports in the tested Unity 6 ARM64 player. Their
    // availability must not depend on selecting GLES at session creation.
    for name in [c"eglChooseConfig", c"eglCreateContext", c"eglCreatePbufferSurface",
        c"eglCreateWindowSurface", c"eglDestroyContext", c"eglDestroySurface",
        c"eglGetConfigAttrib", c"eglGetCurrentContext", c"eglGetCurrentSurface",
        c"eglGetDisplay", c"eglGetError", c"eglGetProcAddress", c"eglInitialize",
        c"eglMakeCurrent", c"eglQueryString", c"eglQuerySurface", c"eglSurfaceAttrib",
        c"eglSwapBuffers", c"eglSwapInterval", c"eglTerminate"] {
        assert_ne!(egl_link_symbol(name), 0, "missing player relocation: {name:?}");
    }
    assert_eq!(egl_link_symbol(c"unrelated_native_symbol"), 0);
}

#[test]
#[ignore = "run alone on a handheld with real Wayland/Mali; this creates a fullscreen GLES session"]
fn handheld_capability_policy_has_coherent_real_gles_queries() {
    use rudroid_common::{
        host_api::{ConfigGetStringFn, HostApiV0},
        registry::HostStr,
    };
    extern "C" fn boolean(key: HostStr<'_>, default: i32) -> i32 {
        if key.to_cstr() == Some(c"gpu.disableProgramBinary") {
            i32::from(std::env::var("RUDROID_TEST_PROGRAM_BINARY").as_deref() != Ok("0"))
        } else {
            default
        }
    }
    extern "C" fn integer(key: HostStr<'_>, default: i64) -> i64 {
        if key.to_cstr() == Some(c"gpu.maxTextureSizeOverride") {
            1024
        } else {
            default
        }
    }
    unsafe extern "C" fn string(
        _key: HostStr<'_>,
        _default: HostStr<'_>,
        out: *mut c_char,
        size: u32,
    ) -> i32 {
        // SAFETY: Host API supplies a writable buffer of size bytes.
        unsafe {
            if size > 0 && !out.is_null() {
                *out = 0;
            }
        }
        0
    }
    let mut api = HostApiV0::stubbed();
    api.config_get_bool = boolean;
    api.config_get_i64 = integer;
    // SAFETY: process-pinned callback writes at most one byte within nonempty output.
    api.config_get_string = unsafe { ConfigGetStringFn::new(string) };
    super::super::gles_policy::configure(&api).unwrap();
    let (mut session, report) = crate::window::create_session_sized(640, 480, 3, 0).unwrap();
    assert_eq!(report.video_driver.as_ref(), c"wayland");
    let disabled = super::super::gles_policy::binary_disabled();
    // SAFETY: creator-thread owned session, no guest threads or retained bindings.
    unsafe {
        session.activate_guest_graphics(None).unwrap();
        let g = active().unwrap();
        let display = g.display as Handle;
        let surface = g.surface as Handle;
        assert_eq!(
            make_current(display, surface, surface, g.context as Handle),
            TRUE
        );
        let native_integer: unsafe extern "C" fn(u32, *mut i32) =
            std::mem::transmute(g.native_address(c"glGetIntegerv"));
        let error: unsafe extern "C" fn() -> u32 =
            std::mem::transmute(g.native_address(c"glGetError"));
        let mut formats = 0;
        native_integer(0x87fe, &mut formats);
        let mut value = [-1, 123];
        identity_integer(0x87fe, value.as_mut_ptr());
        assert_eq!(value, [if disabled { 0 } else { formats }, 123]);
        identity_integer(0x0d33, value.as_mut_ptr());
        assert_eq!(value, [1024, 123]);
        if disabled {
            let mut wide = [-1i64, 456];
            identity_integer64(0x87ff, wide.as_mut_ptr());
            assert_eq!(wide, [0, 456]);
        }
        let mut count = 0;
        identity_integer(0x821d, &mut count);
        assert!(count > 0);
        let text = identity_string(0x1f03);
        assert!(!text.is_null());
        let words = CStr::from_ptr(text.cast())
            .to_str()
            .unwrap()
            .split_ascii_whitespace()
            .collect::<Vec<_>>();
        assert_eq!(count as usize, words.len());
        for (i, word) in words.iter().enumerate() {
            if disabled {
                assert!(!word.contains("program_binary"));
            }
            assert_eq!(
                CStr::from_ptr(identity_string_indexed(0x1f03, i as u32).cast())
                    .to_str()
                    .unwrap(),
                *word
            );
        }
        if disabled {
            assert!(identity_string_indexed(0x1f03, count as u32).is_null());
        }
        let mut length = [999, 77];
        let mut format = [999u32, 88];
        let mut data = [123u8; 16];
        if disabled {
            get_program_binary(
                0,
                16,
                length.as_mut_ptr(),
                format.as_mut_ptr(),
                data.as_mut_ptr().cast(),
            );
            assert_eq!(length, [0, 77]);
            assert_eq!(format, [0, 88]);
            assert_eq!(data, [123; 16]);
            program_binary(0, 0, std::ptr::null(), i32::MAX);
        } else {
            assert_eq!(error(), 0);
            let native_binary: unsafe extern "C" fn(u32, i32, *mut i32, *mut u32, *mut c_void) =
                std::mem::transmute(g.native_address(c"glGetProgramBinary"));
            native_binary(
                u32::MAX,
                16,
                length.as_mut_ptr(),
                format.as_mut_ptr(),
                data.as_mut_ptr().cast(),
            );
            let expected = error();
            assert_ne!(expected, 0);
            get_program_binary(
                u32::MAX,
                16,
                length.as_mut_ptr(),
                format.as_mut_ptr(),
                data.as_mut_ptr().cast(),
            );
            assert_eq!(
                error(),
                expected,
                "disabled=false must forward the real native error"
            );
            assert_eq!(data, [123; 16]);
        }
        assert_eq!(
            g.resolve(c"glad_glGetProgramBinary"),
            g.resolve(c"glGetProgramBinaryOES")
        );
        assert_eq!(
            error(),
            0,
            "compatibility queries must not introduce native GL errors"
        );
        assert_eq!(
            make_current(
                display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            TRUE
        );
    }
}

#[test]
fn diagnostic_captures_are_bounded_and_independent_of_frame_rate() {
    let capture = FrameCapture {
        path: "unused.ppm".into(),
        points: vec![
            CapturePoint::ElapsedMs(50_000),
            CapturePoint::ElapsedMs(56_000),
        ],
        started: Some(std::time::Instant::now()),
        completed: AtomicU32::new(0),
    };
    assert_eq!(capture.take_due(3000, 49_999), None);
    assert_eq!(capture.take_due(3001, 50_005), Some(0));
    assert_eq!(capture.take_due(3002, 50_010), None);
    assert_eq!(capture.take_due(3300, 56_002), Some(1));
    assert_eq!(capture.take_due(3301, 90_000), None);
    let legacy = FrameCapture {
        path: "unused.ppm".into(),
        points: vec![CapturePoint::Swap(120)],
        started: None,
        completed: AtomicU32::new(0),
    };
    assert_eq!(legacy.take_due(119, 99_000), None);
    assert_eq!(legacy.take_due(120, 99_000), Some(0));
    assert_eq!(legacy.take_due(120, 99_000), None);
    assert_eq!(legacy.take_due(121, 99_000), None);
}
thread_local! { static COST: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) }; }
struct Allocator;
fn record(bytes: usize) {
    let _ = COST.try_with(|cost| {
        let (enabled, count, requested) = cost.get();
        if enabled {
            cost.set((true, count + 1, requested + bytes));
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        record(bytes);
        unsafe { System.realloc(ptr, layout, bytes) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
static PRESENTED: AtomicU64 = AtomicU64::new(0);
extern "C" fn presented() {
    PRESENTED.fetch_add(1, Ordering::Relaxed);
}

unsafe fn proc<T: Copy>(name: &CStr) -> T {
    let address = unsafe { rudroid_sdl_guest_symbol(name.as_ptr(), std::ptr::null_mut()) };
    assert_ne!(address, 0, "{}", name.to_string_lossy());
    assert_eq!(address, unsafe { get_proc_address(name.as_ptr()) } as usize);
    unsafe { std::mem::transmute_copy(&address) }
}

#[test]
fn thread_exit_queries_tolerate_destroyed_tls_cache() {
    static COMPLETED: AtomicBool = AtomicBool::new(false);
    struct ExitQuery;
    impl Drop for ExitQuery {
        fn drop(&mut self) {
            assert!(
                CACHED.try_with(|_| ()).is_err(),
                "probe must run after cache destruction"
            );
            let _ = active();
            // With no native current context, this is valid during
            // thread cleanup; release must also tolerate a dead cache.
            assert_eq!(unsafe { release_thread() }, TRUE);
            COMPLETED.store(true, Ordering::Release);
        }
    }
    thread_local! { static EXIT_QUERY: ExitQuery = const { ExitQuery }; }
    std::thread::spawn(|| {
        // Linux destroys these in reverse registration order: a guest
        // TLS destructor may query EGL after the Rust cache was destroyed.
        EXIT_QUERY.with(|_| ());
        let _ = active();
    })
    .join()
    .unwrap();
    assert!(COMPLETED.load(Ordering::Acquire));
}

/// Reuse the existing video-test session: SDL video creation is thread-owned.
pub(crate) fn exercise(session: &mut crate::window::SdlSession) -> Weak<Graphics> {
    // Embedded fullscreen backends grant the measured drawable, while Xvfb
    // normally grants the requested 64x64. Verify the facade against the
    // actual session in both cases, rather than assuming a desktop window.
    let expected_width = session.report().window_w as i32;
    let expected_height = session.report().window_h as i32;
    // SAFETY: creator-thread, exclusively borrowed live session; the
    // static callback only increments an atomic and has process lifetime.
    unsafe { session.activate_guest_graphics(Some(presented)) }.unwrap();
    unsafe { session.activate_guest_graphics(Some(presented)) }.unwrap(); // idempotent
    let g = active().unwrap();
    let weak = Arc::downgrade(&g);
    unsafe {
        assert!(
            current_context().is_null(),
            "activation releases the creating thread's context"
        );
        let display = get_display(std::ptr::null_mut());
        let mut config = std::ptr::null_mut();
        let mut count = 0;
        assert_eq!(
            choose_config(display, std::ptr::null(), &mut config, 1, &mut count),
            TRUE
        );
        assert_eq!(count, 1);
        assert_eq!(
            choose_config(display, std::ptr::null(), &mut config, 0, &mut count),
            TRUE
        );
        assert_eq!(count, 0);
        assert_eq!(
            choose_config(
                display,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
                &mut count
            ),
            TRUE
        );
        assert_eq!(count, 1);
        assert_eq!(
            choose_config(display, std::ptr::null(), &mut config, -1, &mut count),
            FALSE
        );
        assert_eq!(get_error(), BAD_PARAMETER);
        let surface =
            create_window_surface(display, config, 0xfeedusize as Handle, std::ptr::null());
        assert_eq!(
            surface as usize, g.surface,
            "Android native window is never passed to Linux"
        );
        let ctx = create_context(
            display,
            config,
            std::ptr::null_mut(),
            [0x3098, 3, NONE].as_ptr(),
        );
        assert_eq!(ctx as usize, g.context);
        assert_eq!(make_current(display, surface, surface, ctx), TRUE);
        assert_eq!(current_context(), ctx);
        assert_eq!((g.driver.current_context)(), ctx);
        assert_eq!(sdl2::sys::SDL_GL_GetCurrentWindow() as usize, g.window,
            "SDL must own presentation, including native KMSDRM page flips");
        assert_eq!(sdl2::sys::SDL_GL_GetCurrentContext(), ctx);
        assert_eq!(current_surface(DRAW), surface);
        // A legal handle pair with an incompatible read surface must fail
        // without losing either native EGL or SDL's previous binding.
        assert_eq!(make_current(display, surface, std::ptr::null_mut(), ctx), FALSE);
        assert_eq!(get_error(), BAD_MATCH);
        assert_eq!(current_context(), ctx);
        assert_eq!(current_surface(DRAW), surface);
        assert_eq!(sdl2::sys::SDL_GL_GetCurrentWindow() as usize, g.window);
        let mut width = 0;
        assert_eq!(query_surface(display, surface, 0x3057, &mut width), TRUE);
        assert_eq!(width, expected_width);
        let owners = Arc::strong_count(&g);
        let window = native_window_from_surface(std::ptr::null_mut(), 1usize as Handle);
        let second_window = native_window_from_surface(std::ptr::null_mut(), 2usize as Handle);
        assert_eq!(
            window, second_window,
            "Surface facades pin the same Graphics owner"
        );
        assert_eq!(Arc::strong_count(&g), owners + 2);
        native_window_release(second_window);
        assert_eq!(Arc::strong_count(&g), owners + 1);
        assert!(native_window_from_surface(std::ptr::null_mut(), std::ptr::null_mut()).is_null());
        assert_eq!(native_window_width(std::ptr::null_mut()), -22);
        assert_eq!(native_window_width(window), width);
        assert_eq!(native_window_height(window), expected_height);
        assert_eq!(native_window_geometry(window, 0, 0, 0), 0);
        assert_eq!(native_window_geometry(window, 32, 32, 0), -95);
        assert_eq!(native_window_geometry(window, 0, 64, 0), -22);
        assert_eq!(native_window_geometry(window, 0, 0, 1), -95);
        // Every advertised reusable-sync extension must expose its
        // actual driver signal entry; no Android-only handle is forwarded.
        if g.extensions
            .to_str()
            .unwrap()
            .split_ascii_whitespace()
            .any(|ext| ext == "EGL_KHR_reusable_sync")
        {
            let create: unsafe extern "C" fn(Handle, u32, *const i32) -> Handle =
                proc(c"eglCreateSyncKHR");
            let signal: unsafe extern "C" fn(Handle, Handle, u32) -> Bool =
                proc(c"eglSignalSyncKHR");
            let wait: unsafe extern "C" fn(Handle, Handle, i32, u64) -> i32 =
                proc(c"eglClientWaitSyncKHR");
            let destroy: unsafe extern "C" fn(Handle, Handle) -> Bool = proc(c"eglDestroySyncKHR");
            let sync = create(display, 0x30fa, [NONE].as_ptr());
            assert!(!sync.is_null());
            assert_eq!(signal(display, sync, 0x30f2), TRUE);
            assert_eq!(wait(display, sync, 0, 0), 0x30f6);
            assert_eq!(destroy(display, sync), TRUE);
            assert_eq!(get_error(), SUCCESS);
        }
        let clear_color: unsafe extern "C" fn(f32, f32, f32, f32) = proc(c"glClearColor");
        assert_eq!(g.resolve(c"glad_glClearColor"), g.resolve(c"glClearColor"));
        let clear: unsafe extern "C" fn(u32) = proc(c"glClear");
        let read: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void) =
            proc(c"glReadPixels");
        let gl_error: unsafe extern "C" fn() -> u32 = proc(c"glGetError");
        let gen: unsafe extern "C" fn(i32, *mut u32) = proc(c"glGenTextures");
        let bind: unsafe extern "C" fn(u32, u32) = proc(c"glBindTexture");
        let upload: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void) =
            proc(c"glTexImage2D");
        let is_texture: unsafe extern "C" fn(u32) -> u8 = proc(c"glIsTexture");
        let delete: unsafe extern "C" fn(i32, *const u32) = proc(c"glDeleteTextures");
        let mut texture = 0;
        gen(1, &mut texture);
        bind(0x0de1, texture);
        upload(
            0x0de1,
            0,
            0x1908,
            1,
            1,
            0,
            0x1908,
            0x1401,
            [255u8, 0, 0, 255].as_ptr().cast(),
        );
        assert_eq!(is_texture(texture), 1);
        let worker = create_context(display, config, ctx, [0x3098, 3, NONE].as_ptr());
        let pbuffer = create_pbuffer(display, config, [0x3057, 4, 0x3056, 4, NONE].as_ptr());
        assert!(!worker.is_null() && !pbuffer.is_null());
        // Same-thread worker use cannot leave SDL believing its window is
        // still bound; otherwise a later SDL bind can incorrectly be skipped.
        assert_eq!(make_current(display, pbuffer, pbuffer, worker), TRUE);
        assert!(sdl2::sys::SDL_GL_GetCurrentWindow().is_null());
        assert_eq!(current_surface(DRAW), pbuffer);
        assert_eq!(current_context(), worker);
        assert_eq!(make_current(display, surface, surface, ctx), TRUE);
        assert_eq!(current_surface(DRAW), surface);
        assert_eq!(sdl2::sys::SDL_GL_GetCurrentWindow() as usize, g.window);
        assert_eq!(get_error(), SUCCESS);
        let handles = (display as usize, worker as usize, pbuffer as usize);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (display, worker, pbuffer) = handles;
            assert_eq!(
                make_current(
                    display as Handle,
                    pbuffer as Handle,
                    pbuffer as Handle,
                    worker as Handle
                ),
                TRUE
            );
            assert_eq!(current_context() as usize, worker);
            assert_eq!(
                is_texture(texture),
                1,
                "worker sees the same driver texture"
            );
            assert_eq!(
                initialize(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                FALSE
            );
            assert_eq!(get_error(), BAD_DISPLAY);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert_eq!(release_thread(), TRUE);
            assert!(current_context().is_null());
        });
        started_rx.recv().unwrap();
        assert_eq!(
            current_context(),
            ctx,
            "other thread binding does not replace this thread's binding"
        );
        assert_eq!(get_error(), SUCCESS, "EGL errors remain thread-local");
        release_tx.send(()).unwrap();
        thread.join().unwrap();
        assert_eq!(destroy_context(display, worker), TRUE);
        assert_eq!(destroy_surface(display, pbuffer), TRUE);
        delete(1, &texture);
        assert_eq!(is_texture(texture), 0);
        let cached_entries = g.addresses.lock().unwrap().len();
        for _ in 0..100 {
            assert_eq!(g.resolve(c"eglUnsupportedRudroidFunction"), 0);
            assert_eq!(g.resolve(c"gl_invalid_name"), 0);
        }
        assert_eq!(
            g.addresses.lock().unwrap().len(),
            cached_entries,
            "unsupported lookups must not grow the cache"
        );
        COST.with(|cost| cost.set((true, 0, 0)));
        for _ in 0..10_000 {
            assert_eq!(
                get_proc_address(c"glClearColor".as_ptr()) as usize,
                clear_color as usize
            );
            assert_eq!(get_display(std::ptr::null_mut()), display);
            assert_eq!(current_context(), ctx);
        }
        let (_, allocations, bytes) = COST.with(|cost| cost.replace((false, 0, 0)));
        assert_eq!((allocations, bytes), (0, 0));
        println!("GUEST_GRAPHICS_COST {{\"queries\":30000,\"rust_allocations\":{allocations},\"requested_bytes\":{bytes}}}");
        COST.with(|cost| cost.set((true, 0, 0)));
        for _ in 0..10_000 {
            assert_eq!(native_window_width(window), expected_width);
            assert_eq!(native_window_height(window), expected_height);
            native_window_acquire(window);
            native_window_release(window);
        }
        let (_, allocations, bytes) = COST.with(|cost| cost.replace((false, 0, 0)));
        assert_eq!((allocations, bytes), (0, 0));
        assert_eq!(Arc::strong_count(&g), owners + 1);
        println!("NATIVE_WINDOW_COST {{\"calls\":40000,\"rust_allocations\":{allocations},\"requested_bytes\":{bytes}}}");
        clear_color(0.25, 0.5, 0.75, 1.0);
        clear(0x4000);
        let mut pixel = [0u8; 4];
        read(16, 16, 1, 1, 0x1908, 0x1401, pixel.as_mut_ptr().cast());
        assert!(
            pixel[0].abs_diff(64) <= 1
                && pixel[1].abs_diff(128) <= 1
                && pixel[2].abs_diff(191) <= 1,
            "{pixel:?}"
        );
        assert_eq!(gl_error(), 0);
        assert_eq!(swap(display, surface), TRUE);
        assert_eq!(PRESENTED.load(Ordering::Relaxed), 1);
        // Leave real driver objects for service shutdown, then query them
        // while SDL's display still exists: proves scoped cleanup itself.
        let leftover = create_context(display, config, ctx, [0x3098, 3, NONE].as_ptr());
        let leftover_surface =
            create_pbuffer(display, config, [0x3057, 4, 0x3056, 4, NONE].as_ptr());
        assert!(!leftover.is_null() && !leftover_surface.is_null());
        assert_eq!(release_thread(), TRUE);
        g.shutdown();
        assert_eq!(native_window_width(window), -19);
        assert_eq!(native_window_height(window), -19);
        assert_eq!(native_window_geometry(window, 0, 0, 0), -19);
        assert!(native_window_from_surface(std::ptr::null_mut(), 1usize as Handle).is_null());
        native_window_release(window);
        assert_eq!(Arc::strong_count(&g), owners);
        let mut value = 0;
        assert_eq!(
            (g.driver.query_context)(display, leftover, 0x3098, &mut value),
            FALSE
        );
        assert_eq!(
            (g.driver.query_surface)(display, leftover_surface, 0x3057, &mut value),
            FALSE
        );
        assert_eq!(
            rudroid_sdl_guest_symbol(c"glClear".as_ptr(), std::ptr::null_mut()),
            0
        );
        assert!(get_display(std::ptr::null_mut()).is_null());
        assert_eq!(get_error(), NOT_INITIALIZED);
        assert_eq!(get_error(), SUCCESS);
        println!("[GUEST GRAPHICS] real pixel={pixel:?}; shared worker/texture, TLS binding, cleanup and 30000 allocation-free queries verified");
    }
    weak
}

/// Real GLES regression: both present paths fill the physical output and preserve guest state.
pub(crate) fn exercise_render_scaling() {
    for (width, height, sharp) in [(48, 48, 1), (32, 32, 0), (48, 32, 1)] {
        let (mut session, _) = crate::window::create_session_sized(64, 64, 3, 2).unwrap();
        assert!(session.configure_render(0, 48, 1).is_err());
        assert!(session.configure_render(width, height, 2).is_err());
        session.configure_render(width, height, sharp).unwrap();
        // SAFETY: the live session belongs to this creating thread and no guest is running.
        unsafe { session.activate_guest_graphics(None) }.unwrap();
        assert!(session.configure_render(width, height, sharp).is_err());
        let g = active().unwrap();
        let scale = g.scale.as_ref().unwrap();
        let display = g.display as Handle;
        let surface = g.surface as Handle;
        let context = g.context as Handle;
        let window = unsafe { native_window_from_surface(std::ptr::null_mut(), 1usize as Handle) };
        unsafe {
            assert_eq!(query_surface(display, surface, WIDTH, &mut 0), TRUE);
            let mut actual = 0;
            query_surface(display, surface, WIDTH, &mut actual);
            assert_eq!(actual, width);
            query_surface(display, surface, HEIGHT, &mut actual);
            assert_eq!(actual, height);
            assert_eq!(
                (native_window_width(window), native_window_height(window)),
                (width, height)
            );
            assert_eq!(make_current(display, surface, surface, context), TRUE);
            // Compile/link both real presentation variants only after making the context current.
            for half_precision in [false, true] {
                let program = crate::render_scale::program(&scale.gl, half_precision).unwrap();
                (scale.gl.delete_program)(program);
            }
            let get: unsafe extern "C" fn(u32, *mut i32) = proc(c"glGetIntegerv");
            let bind: unsafe extern "C" fn(u32, u32) = proc(c"glBindFramebuffer");
            let clear_color: unsafe extern "C" fn(f32, f32, f32, f32) = proc(c"glClearColor");
            let clear: unsafe extern "C" fn(u32) = proc(c"glClear");
            let read: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void) =
                proc(c"glReadPixels");
            get(0x8ca6, &mut actual);
            assert_eq!(actual, 0);
            (scale.gl.get)(0x8ca6, &mut actual);
            let owned = actual as u32;
            assert_ne!(owned, 0);
            let get64: unsafe extern "C" fn(u32, *mut i64) = proc(c"glGetInteger64v");
            let mut wide = -1;
            get64(0x8ca6, &mut wide);
            assert_eq!(wide, 0);
            let get_float: unsafe extern "C" fn(u32, *mut f32) = proc(c"glGetFloatv");
            let mut float = -1.0;
            get_float(0x8ca6, &mut float);
            assert_eq!(float, 0.0);
            let get_bool: unsafe extern "C" fn(u32, *mut u8) = proc(c"glGetBooleanv");
            let mut boolean = 1;
            get_bool(0x8ca6, &mut boolean);
            assert_eq!(boolean, 0);
            let buffers: unsafe extern "C" fn(i32, *const u32) = proc(c"glDrawBuffers");
            buffers(1, [0x0405].as_ptr());
            let read_buffer: unsafe extern "C" fn(u32) = proc(c"glReadBuffer");
            read_buffer(0x0405);
            get(0x0c02, &mut actual);
            assert_eq!(actual, 0x0405);
            clear_color(0.25, 0.5, 0.75, 1.0);
            clear(0x4000);
            // Exercise the unchanged-state branches before the restrictive snapshot below.
            scale.present();
            assert_eq!((scale.gl.error)(), 0);
            get(0x84e0, &mut actual);
            assert_eq!(actual, 0x84c0);
            let caps = [0x0be2, 0x0b44, 0x0b71, 0x8c89, 0x0c11, 0x0b90];
            for cap in caps { assert_eq!((scale.gl.enabled)(cap), 0); }

            // A distinct read framebuffer must survive both sharp and blit presentation.
            let mut read_fbo = 0;
            (scale.gl.gen_framebuffer)(1, &mut read_fbo);
            assert_ne!(read_fbo, 0);
            bind(0x8ca8, read_fbo);
            let gen_samplers: unsafe extern "C" fn(i32, *mut u32) = proc(c"glGenSamplers");
            let delete_samplers: unsafe extern "C" fn(i32, *const u32) = proc(c"glDeleteSamplers");
            let mut guest_sampler = 0;
            gen_samplers(1, &mut guest_sampler);
            assert_ne!(guest_sampler, 0);
            (scale.gl.bind_sampler)(0, guest_sampler);
            // Host presentation must ignore restrictive guest state, then restore all of it.
            (scale.gl.viewport)(3, 4, 17, 19);
            (scale.gl.scissor)(2, 3, 1, 1);
            (scale.gl.color_mask)(1, 0, 1, 0);
            for cap in caps { (scale.gl.enable)(cap); }
            (scale.gl.active_texture)(0x84c3);
            let mut previous = [0; 4];
            (scale.gl.get)(0x0ba2, previous.as_mut_ptr());
            // Native vector queries must retain their full width through the scaled facade.
            let mut vector = [-77; 6];
            get(0x0ba2, vector.as_mut_ptr().add(1));
            assert_eq!(vector, [-77, 3, 4, 17, 19, -77]);
            let mut wide_vector = [-77; 6];
            get64(0x0ba2, wide_vector.as_mut_ptr().add(1));
            assert_eq!(wide_vector, [-77, 3, 4, 17, 19, -77]);
            scale.present();
            let mut now = [0; 4];
            (scale.gl.get)(0x0ba2, now.as_mut_ptr());
            assert_eq!(previous, now);
            get(0x84e0, &mut actual);
            assert_eq!(actual, 0x84c3);
            let mut mask = [0; 4];
            get_bool(0x0c23, mask.as_mut_ptr());
            assert_eq!(mask, [1, 0, 1, 0]);
            for cap in caps { assert_eq!((scale.gl.enabled)(cap), 1); }
            get(0x8ca6, &mut actual);
            assert_eq!(actual, 0);
            get(0x8caa, &mut actual);
            assert_eq!(actual as u32, read_fbo);
            (scale.gl.active_texture)(0x84c0);
            get(0x8919, &mut actual);
            assert_eq!(actual as u32, guest_sampler);
            (scale.gl.bind_sampler)(0, 0);
            delete_samplers(1, &guest_sampler);
            (scale.gl.active_texture)(0x84c3);
            // Raw native read binds physical framebuffer zero, bypassing the logical-default wrapper.
            (scale.gl.bind)(super::super::render_scale::READ, 0);
            let mut pixel = [0u8; 4];
            read(61, 61, 1, 1, 0x1908, 0x1401, pixel.as_mut_ptr().cast());
            assert!(
                pixel[0].abs_diff(64) <= 1
                    && pixel[1].abs_diff(128) <= 1
                    && pixel[2].abs_diff(191) <= 1,
                "{pixel:?}"
            );
            bind(0x8ca8, 0);
            (scale.gl.delete_framebuffer)(1, &read_fbo);
            let delete: unsafe extern "C" fn(i32, *const u32) = proc(c"glDeleteFramebuffers");
            delete(1, &owned);
            assert_eq!(
                (scale.gl.is_framebuffer)(owned),
                1,
                "guest deletion cannot free host framebuffer"
            );
            let worker = create_context(
                display,
                g.config as Handle,
                context,
                [0x3098, 3, NONE].as_ptr(),
            );
            let pbuffer = create_pbuffer(
                display,
                g.config as Handle,
                [WIDTH, 4, HEIGHT, 4, NONE].as_ptr(),
            );
            assert!(!worker.is_null() && !pbuffer.is_null());
            assert_eq!(make_current(display, pbuffer, pbuffer, worker), TRUE);
            get(0x8ca6, &mut actual);
            assert_eq!(actual, 0, "worker defaults must remain native");
            let mut worker_fbo = 0;
            (scale.gl.gen_framebuffer)(1, &mut worker_fbo);
            bind(0x8d40, worker_fbo);
            get(0x8ca6, &mut actual);
            assert_eq!(actual as u32, worker_fbo);
            delete(1, &worker_fbo);
            assert_eq!(
                (scale.gl.is_framebuffer)(worker_fbo),
                0,
                "context-local worker name may collide with private primary name"
            );
            assert_eq!(make_current(display, surface, surface, context), TRUE);
            assert_eq!((scale.gl.is_framebuffer)(owned), 1);
            assert_eq!(destroy_context(display, worker), TRUE);
            assert_eq!(destroy_surface(display, pbuffer), TRUE);
            let mut guest = 0;
            (scale.gl.gen_framebuffer)(1, &mut guest);
            bind(0x8d40, guest);
            delete(1, &guest);
            get(0x8ca6, &mut actual);
            assert_eq!(actual, 0);
            get(0x8caa, &mut actual);
            assert_eq!(actual, 0);
            assert_eq!((scale.gl.error)(), 0);
            assert_eq!(release_thread(), TRUE);
            g.shutdown();
            g.shutdown(); // idempotent: no double deletion on session Drop.
            assert_eq!(
                (g.driver.make_current)(display, surface, surface, context),
                TRUE
            );
            assert_eq!(
                (scale.gl.is_framebuffer)(owned),
                0,
                "private GPU object is deleted before SDL destruction"
            );
            native_window_release(window);
            assert_eq!(
                (g.driver.make_current)(
                    display,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                TRUE
            );
        }
        drop(g);
        drop(session);
    }
}

#[test]
#[ignore = "run alone on real handheld Wayland/Mali; exercises source/compile/link/bind/context deletion"]
fn handheld_shader_policy_tracks_the_actual_linked_executable() {
    use rudroid_common::{host_api::HostApiV0, registry::HostStr};
    extern "C" fn boolean(key: HostStr<'_>, default: i32) -> i32 {
        match key.to_cstr() {
            Some(k)
                if k == c"gpu.disableScreenRefraction" || k == c"gpu.skipDisabledShaderDraws" =>
            {
                1
            }
            _ => default,
        }
    }
    let mut api = HostApiV0::stubbed();
    api.config_get_bool = boolean;
    super::super::shader_policy::configure(&api);
    let (mut session, _) = crate::window::create_session_sized(640, 480, 3, 0).unwrap();
    // SAFETY: single creator thread, exclusive current real context; all pointer spans and
    // native function prototypes below are exact GLES contracts, resources remain live.
    unsafe {
        session.activate_guest_graphics(None).unwrap();
        let g = active().unwrap();
        let display = g.display as Handle;
        let surface = g.surface as Handle;
        let primary = g.context as Handle;
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        macro_rules! gl {
            ($name:literal,$ty:ty) => {
                std::mem::transmute::<usize, $ty>(g.resolve($name))
            };
        }
        let error = gl!(c"glGetError", extern "C" fn() -> u32);
        let create_shader = gl!(c"glCreateShader", extern "C" fn(u32) -> u32);
        let source = gl!(
            c"glShaderSource",
            extern "C" fn(u32, i32, *const *const c_char, *const i32)
        );
        let compile = gl!(c"glCompileShader", extern "C" fn(u32));
        let shader_query = gl!(c"glGetShaderiv", extern "C" fn(u32, u32, *mut i32));
        let create_program = gl!(c"glCreateProgram", extern "C" fn() -> u32);
        let attach = gl!(c"glAttachShader", extern "C" fn(u32, u32));
        let link = gl!(c"glLinkProgram", extern "C" fn(u32));
        let use_program = gl!(c"glUseProgram", extern "C" fn(u32));
        let program_query = gl!(c"glGetProgramiv", extern "C" fn(u32, u32, *mut i32));
        let delete_shader = gl!(c"glDeleteShader", extern "C" fn(u32));
        let draw = gl!(c"glDrawArrays", extern "C" fn(u32, i32, i32));
        let clear_color = gl!(c"glClearColor", extern "C" fn(f32, f32, f32, f32));
        let clear = gl!(c"glClear", extern "C" fn(u32));
        let read = gl!(
            c"glReadPixels",
            extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void)
        );
        let submit = |shader: u32, text: &CStr| {
            let ptr = text.as_ptr();
            source(shader, 1, &ptr, std::ptr::null());
        };
        let vertex = create_shader(0x8b31);
        submit(
            vertex,
            c"attribute vec2 position; void main(){gl_Position=vec4(position,0.0,1.0);}",
        );
        compile(vertex);
        let fragment = create_shader(0x8b30);
        let marked=c"precision mediump float; uniform sampler2D _GrabTexture; uniform float _BumpAmt; void main(){gl_FragColor=vec4(1.0,0.0,0.0,1.0);}";
        submit(fragment, marked);
        compile(fragment);
        let mut ok = 0;
        shader_query(fragment, 0x8b81, &mut ok);
        assert_eq!(ok, 1);
        let program = create_program();
        attach(program, vertex);
        attach(program, fragment);
        delete_shader(fragment);
        link(program);
        program_query(program, 0x8b82, &mut ok);
        assert_eq!(ok, 1);
        use_program(program);
        let count = shader_gl::skipped_draws();
        draw(4, 0, 3);
        assert_eq!(shader_gl::skipped_draws(), count + 1);
        let normal = c"precision mediump float; void main(){gl_FragColor=vec4(1.0,0.0,0.0,1.0);}";
        submit(fragment, normal);
        link(program);
        draw(4, 0, 3);
        assert_eq!(
            shader_gl::skipped_draws(),
            count + 2,
            "pending source must not replace the compiled discard shader"
        );
        compile(fragment);
        draw(4, 0, 3);
        assert_eq!(
            shader_gl::skipped_draws(),
            count + 3,
            "compile alone must not replace the linked program"
        );
        link(program);
        let gen = gl!(c"glGenBuffers", extern "C" fn(i32, *mut u32));
        let bind = gl!(c"glBindBuffer", extern "C" fn(u32, u32));
        let data = gl!(
            c"glBufferData",
            extern "C" fn(u32, isize, *const c_void, u32)
        );
        let location = gl!(
            c"glGetAttribLocation",
            extern "C" fn(u32, *const c_char) -> i32
        );
        let enable = gl!(c"glEnableVertexAttribArray", extern "C" fn(u32));
        let pointer = gl!(
            c"glVertexAttribPointer",
            extern "C" fn(u32, i32, u32, u8, i32, *const c_void)
        );
        let mut buffer = 0;
        gen(1, &mut buffer);
        bind(0x8892, buffer);
        let vertices = [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0];
        data(
            0x8892,
            std::mem::size_of_val(&vertices) as isize,
            vertices.as_ptr().cast(),
            0x88e4,
        );
        let attribute = location(program, c"position".as_ptr());
        assert!(attribute >= 0);
        enable(attribute as u32);
        pointer(attribute as u32, 2, 0x1406, 0, 0, std::ptr::null());
        clear_color(0.0, 1.0, 0.0, 1.0);
        clear(0x4000);
        draw(4, 0, 3);
        let mut pixel = [0u8; 4];
        read(10, 10, 1, 1, 0x1908, 0x1401, pixel.as_mut_ptr().cast());
        assert_eq!(pixel, [255, 0, 0, 255]);
        assert_eq!(shader_gl::skipped_draws(), count + 3);
        submit(fragment, c"#version invalid\nvoid main(){}");
        compile(fragment);
        shader_query(fragment, 0x8b81, &mut ok);
        assert_eq!(ok, 0);
        link(program);
        program_query(program, 0x8b82, &mut ok);
        assert_eq!(ok, 0);
        draw(4, 0, 3);
        assert_eq!(
            shader_gl::skipped_draws(),
            count + 3,
            "failed compile/link preserves the bound executable"
        );
        use_program(u32::MAX);
        assert_ne!(error(), 0);
        draw(4, 0, 3);
        assert_eq!(
            shader_gl::skipped_draws(),
            count + 3,
            "invalid program cannot poison the current binding"
        );
        assert_eq!(error(), 0);
        let second = create_shader(0x8b30);
        submit(second, marked);
        compile(second);
        let skipped_program = create_program();
        attach(skipped_program, vertex);
        attach(skipped_program, second);
        link(skipped_program);
        use_program(skipped_program);
        draw(4, 0, 3);
        let before_switch = shader_gl::skipped_draws();
        let regular = create_shader(0x8b30);
        submit(regular, normal);
        compile(regular);
        let regular_program = create_program();
        attach(regular_program, vertex);
        attach(regular_program, regular);
        link(regular_program);
        use_program(regular_program);
        // New shared context starts with program 0 and must not inherit this thread's skip flag.
        g.primary_claimed.store(true, Ordering::Release);
        let attrs = [0x3098, 3, NONE];
        let worker = create_context(display, g.config as Handle, primary, attrs.as_ptr());
        assert!(!worker.is_null() && worker != primary);
        assert_eq!(make_current(display, surface, surface, worker), TRUE);
        use_program(skipped_program);
        draw(4, 0, 3);
        assert_eq!(shader_gl::skipped_draws(), before_switch + 1);
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        draw(4, 0, 3);
        assert_eq!(
            shader_gl::skipped_draws(),
            before_switch + 1,
            "primary context must restore its own ordinary program after a worker skips draws"
        );
        assert_eq!(destroy_context(display, worker), TRUE);
        use_program(0);
        assert_eq!(error(), 0);
        assert_eq!(
            make_current(
                display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            TRUE
        );
    }
}

#[test]
#[ignore = "run alone on real handheld Wayland/Mali; validates translated ES100 source and compilation"]
fn handheld_gles2_translation_compiles_both_stages() {
    use rudroid_common::{host_api::HostApiV0, registry::HostStr};
    extern "C" fn boolean(key: HostStr<'_>, default: i32) -> i32 {
        if key.to_cstr() == Some(c"gpu.shaderGles2Rewrite") {
            1
        } else {
            default
        }
    }
    let mut api = HostApiV0::stubbed();
    api.config_get_bool = boolean;
    super::super::shader_policy::configure(&api);
    let (mut session, _) = crate::window::create_session_sized(640, 480, 2, 0).unwrap();
    // SAFETY: exclusive real creator-thread context, exact GLES prototypes, static readable
    // sources and bounded query buffers; no guest workers or retained native pointers.
    unsafe {
        session.activate_guest_graphics(None).unwrap();
        let g = active().unwrap();
        assert_eq!(
            make_current(
                g.display as Handle,
                g.surface as Handle,
                g.surface as Handle,
                g.context as Handle
            ),
            TRUE
        );
        let create: extern "C" fn(u32) -> u32 = std::mem::transmute(g.resolve(c"glCreateShader"));
        let source: extern "C" fn(u32, i32, *const *const c_char, *const i32) =
            std::mem::transmute(g.resolve(c"glShaderSource"));
        let compile: extern "C" fn(u32) = std::mem::transmute(g.resolve(c"glCompileShader"));
        let q: extern "C" fn(u32, u32, *mut i32) = std::mem::transmute(g.resolve(c"glGetShaderiv"));
        let get: extern "C" fn(u32, i32, *mut i32, *mut c_char) =
            std::mem::transmute(g.resolve(c"glGetShaderSource"));
        let delete: extern "C" fn(u32) = std::mem::transmute(g.resolve(c"glDeleteShader"));
        for (kind,text) in [(0x8b31,c"#version 300 es\nlayout(location=0) in vec2 position;\nout vec2 uv;\nvoid main(){uv=position; gl_Position=vec4(position,0.0,1.0);}"),(0x8b30,c"#version 300 es\nprecision mediump float;\nin vec2 uv;\nout vec4 frag;\nvoid main(){frag = vec4(uv,0.0,1.0);}")]{
            let shader=create(kind);let ptr=text.as_ptr();source(shader,1,&ptr,std::ptr::null());let mut data=[0u8;1024];let mut len=0;get(shader,1024,&mut len,data.as_mut_ptr().cast());assert!(std::str::from_utf8(&data[..len as usize]).unwrap().starts_with("#version 100"));compile(shader);let mut compiled=0;q(shader,0x8b81,&mut compiled);assert_eq!(compiled,1);delete(shader);
        }
        assert_eq!(
            make_current(
                g.display as Handle,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            TRUE
        );
    }
}
