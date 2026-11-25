use super::*;
use super::super::{active, create_context, create_pbuffer, destroy_context, destroy_surface, get_proc_address,
                   geometry_trace, make_current, Handle, NONE, TRUE};
use std::{cell::Cell, ffi::c_void, ptr::null_mut};

const COPY_READ: u32 = 0x8f36;
const COPY_WRITE: u32 = 0x8f37;
const RGBA8: u32 = 0x8058;
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
type Attach = extern "C" fn(u32, u32, u32);
type AttachRange = extern "C" fn(u32, u32, u32, isize, isize);

thread_local! { static STORES: Cell<(u32, u32)> = const { Cell::new((0, 0)) }; }

macro_rules! native {
    ($g:expr, $name:literal, $ty:ty) => {{
        let address = $g.native_address($name);
        assert_ne!(address, 0);
        std::mem::transmute::<usize, $ty>(address)
    }};
}

fn materialize(buffer: u32) -> bool {
    let (logical, storage) = STORES.get();
    assert_eq!(buffer, logical);
    let g = active().unwrap();
    // SAFETY: the fixture owns both stores and the primary context;
    // native GLES calls use exact signatures and local out slots.
    unsafe {
        let get = native!(g, c"glGetIntegerv", GetInteger);
        let bind = native!(g, c"glBindBuffer", extern "C" fn(u32, u32));
        let copy = native!(g, c"glCopyBufferSubData", extern "C" fn(u32, u32, isize, isize, isize));
        let mut read = 0;
        let mut write = 0;
        get(COPY_READ, &mut read);
        get(COPY_WRITE, &mut write);
        bind(COPY_READ, storage);
        bind(COPY_WRITE, logical);
        copy(COPY_READ, COPY_WRITE, 0, 0, 32);
        bind(COPY_READ, read as u32);
        bind(COPY_WRITE, write as u32);
    }
    true
}

fn setup_sampler(g: &Graphics) {
    // SAFETY: owner test context, process-pinned pointers with exact GLES
    // signatures and NUL-terminated static shader source.
    unsafe {
        let create = native!(g, c"glCreateShader", extern "C" fn(u32) -> u32);
        let source = native!(g, c"glShaderSource", extern "C" fn(u32, i32, *const *const c_char, *const i32));
        let compile = native!(g, c"glCompileShader", extern "C" fn(u32));
        let shader_iv = native!(g, c"glGetShaderiv", extern "C" fn(u32, u32, *mut i32));
        let program = native!(g, c"glCreateProgram", extern "C" fn() -> u32)();
        let attach = native!(g, c"glAttachShader", extern "C" fn(u32, u32));
        for (kind, text) in [
            (0x8b31, c"#version 320 es\nconst vec2 p[3]=vec2[3](vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));void main(){gl_Position=vec4(p[gl_VertexID],0.,1.);}"),
            (0x8b30, c"#version 320 es\nprecision highp float;uniform highp samplerBuffer src;out vec4 col;void main(){col=texelFetch(src,0);}"),
        ] {
            let shader = create(kind);
            source(shader, 1, &text.as_ptr(), std::ptr::null());
            compile(shader);
            let mut ok = 0;
            shader_iv(shader, 0x8b81, &mut ok);
            assert_eq!(ok, 1);
            attach(program, shader);
        }
        let mut ok = 0;
        native!(g, c"glLinkProgram", extern "C" fn(u32))(program);
        native!(g, c"glGetProgramiv", extern "C" fn(u32, u32, *mut i32))(program, 0x8b82, &mut ok);
        assert_eq!(ok, 1);
        native!(g, c"glUseProgram", extern "C" fn(u32))(program);
    }
}

pub(crate) fn cached_named_aliases_use_native_bytes_and_lifetimes() {
    geometry_trace::enable_alias_fixture();
    let (mut session, report) = crate::window::create_session_sized(64, 64, 3, 2).unwrap();
    // SAFETY: the test owns the window, EGL contexts and buffers. Local out
    // buffers and forwarded function pointers match their exact GL signatures.
    unsafe {
        session.activate_guest_graphics(None).unwrap();
        let g = active().unwrap();
        let display = g.display as Handle;
        let surface = g.surface as Handle;
        let primary = g.context as Handle;
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        let guard = g.geometry_trace.as_ref().unwrap().alias.as_ref().unwrap();
        assert!(guard.texture_buffer(), "driver version = {:?}", report.gl_version);
        let group = Guard::context(&g);
        let identity = |buffer| Name { group, buffer };
        let state = |buffer| guard.with(|s| s.objects[&identity(buffer)]);
        let promote = |buffer| guard.with(|s| {
            if !s.closed { return false; }
            let object = s.objects.get_mut(&identity(buffer)).unwrap();
            if object.aliased { return false; }
            object.resident = true;
            true
        });
        let bind: extern "C" fn(u32, u32) = std::mem::transmute(g.resolve(c"glBindBuffer"));
        let delete: extern "C" fn(i32, *const u32) = std::mem::transmute(g.resolve(c"glDeleteBuffers"));
        let bind_texture = native!(g, c"glBindTexture", extern "C" fn(u32, u32));
        let generate = native!(g, c"glGenBuffers", extern "C" fn(i32, *mut u32));
        let data = native!(g, c"glBufferData", extern "C" fn(u32, isize, *const c_void, u32));
        let error = native!(g, c"glGetError", extern "C" fn() -> u32);
        let draw = native!(g, c"glDrawArrays", extern "C" fn(u32, i32, i32));
        let read = native!(g, c"glReadPixels", extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void));
        let mut textures = [0; 5];
        native!(g, c"glGenTextures", extern "C" fn(i32, *mut u32))(5, textures.as_mut_ptr());
        let define = |name, pixel: &[u8; 4]| {
            bind(COPY_WRITE, name);
            let mut payload = [0; 32];
            payload[..4].copy_from_slice(pixel);
            data(COPY_WRITE, 32, payload.as_ptr().cast(), 0x88e8);
        };
        let create = |pixel: &[u8; 4]| {
            let mut name = 0;
            generate(1, &mut name);
            define(name, pixel);
            name
        };
        let sample = |texture| {
            bind_texture(TEXTURE_BUFFER, texture);
            draw(4, 0, 3);
            let mut pixel = [0; 4];
            read(32, 32, 1, 1, 0x1908, 0x1401, pixel.as_mut_ptr().cast());
            assert_eq!(error(), 0);
            pixel
        };

        assert_eq!(make_current(display, null_mut(), null_mut(), null_mut()), TRUE);
        assert_eq!(Guard::context(&g), 0);
        let full = get_proc_address(c"glTexBuffer".as_ptr()) as usize;
        let range = get_proc_address(c"glTexBufferRange".as_ptr()) as usize;
        assert_ne!(full, 0);
        assert_ne!(range, 0);
        assert_ne!(full, g.native_address(c"glTexBuffer"));
        assert_eq!(full, g.resolve(c"glad_glTexBuffer"));
        assert_eq!(range, g.resolve(c"glTexBufferRange"));
        let full: Attach = std::mem::transmute(full);
        let range: AttachRange = std::mem::transmute(range);
        let mut aliases = Vec::new();
        for (name, range) in [(c"glTexBufferEXT", false), (c"glTexBufferRangeEXT", true),
                              (c"glTexBufferOES", false), (c"glTexBufferRangeOES", true)] {
            let address = get_proc_address(name.as_ptr()) as usize;
            assert_ne!(address, 0);
            assert_ne!(address, g.native_address(name));
            assert_eq!(address, g.resolve(name));
            aliases.push((address, range));
        }
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        setup_sampler(&g);

        // The same public trampoline is usable with wholly native storage.
        let old = create(&RED);
        let original = state(old).generation;
        assert!(!promote(old));
        bind_texture(TEXTURE_BUFFER, textures[0]);
        full(TEXTURE_BUFFER, RGBA8, old);
        assert_eq!(sample(textures[0]), RED);
        assert!(state(old).aliased);
        assert_eq!(guard.with(|s| (s.attempted, s.confirmed, s.changes)), (1, 1, 1));
        full(0, RGBA8, old);
        assert_eq!(error(), 0x0500);

        *guard.materializer.lock().unwrap() = Some(materialize);
        guard.with(|s| s.closed = true); // restricted owned fixture, never set by KTC
        assert!(!promote(old)); // prior static alias
        full(TEXTURE_BUFFER, RGBA8, 0);
        assert!(!promote(old)); // detach does not turn a prior alias into closed ownership
        delete(1, &old);
        assert!(!guard.with(|s| s.objects.contains_key(&identity(old))));
        // Rebind the previously deleted name explicitly. Mesa's allocator
        // need not pick the lowest free name, but the native bind recreates it.
        let logical = old;
        define(logical, &RED);
        assert_eq!(error(), 0);
        assert_ne!(state(logical).generation, original);
        assert!(!state(logical).aliased);
        let storage = create(&GREEN);
        STORES.set((logical, storage));
        assert!(promote(logical));
        bind_texture(TEXTURE_BUFFER, textures[1]);
        // Cached without a current context. It must copy before attaching.
        full(TEXTURE_BUFFER, RGBA8, logical);
        assert_eq!(sample(textures[1]), GREEN);
        bind_texture(TEXTURE_BUFFER, textures[3]);
        range(TEXTURE_BUFFER, RGBA8, logical, 0, 4);
        assert_eq!(sample(textures[3]), GREEN);
        for (address, is_range) in aliases {
            bind_texture(TEXTURE_BUFFER, textures[4]);
            if is_range { std::mem::transmute::<usize, AttachRange>(address)(TEXTURE_BUFFER, RGBA8, logical, 0, 4); }
            else { std::mem::transmute::<usize, Attach>(address)(TEXTURE_BUFFER, RGBA8, logical); }
            assert_eq!(sample(textures[4]), GREEN);
        }
        assert!(!state(logical).resident);
        assert!(!promote(logical));
        assert_eq!(guard.with(|s| (s.retired, s.materialized)), (1, 1));

        // A raw unknown entry cannot be published while owner materialization
        // needs the current primary context. A retry with ownership can fall back.
        let raw_candidate = c"glDrawElementsInstancedBaseVertex";
        let expected = g.native_address(raw_candidate);
        assert_ne!(expected, 0);
        let shared_buffer = create(&RED);
        STORES.set((shared_buffer, storage));
        assert!(promote(shared_buffer));
        assert_eq!(make_current(display, null_mut(), null_mut(), null_mut()), TRUE);
        assert!(get_proc_address(raw_candidate.as_ptr()).is_null());
        assert_eq!(make_current(display, surface, surface, primary), TRUE);

        // Exposure of a shared context also needs the native store ready.
        // Neither the blocked lookup nor context teardown can reopen admission.
        let attrs = [0x3098, 3, NONE];
        let shared = create_context(display, g.config as Handle, primary, attrs.as_ptr());
        assert!(!shared.is_null());
        assert!(!state(shared_buffer).resident);
        assert_eq!(get_proc_address(raw_candidate.as_ptr()) as usize, expected);
        let pbuffer = create_pbuffer(display, g.config as Handle, [0x3057, 64, 0x3056, 64, NONE].as_ptr());
        assert!(!pbuffer.is_null());
        assert_eq!(make_current(display, pbuffer, pbuffer, shared), TRUE);
        assert_eq!(Guard::context(&g), group);
        bind_texture(TEXTURE_BUFFER, textures[2]);
        full(TEXTURE_BUFFER, RGBA8, shared_buffer);
        assert_eq!(error(), 0);
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        assert_eq!(sample(textures[2]), GREEN);
        assert!(!promote(shared_buffer));
        assert_eq!(destroy_context(display, shared), TRUE);
        assert_eq!(destroy_surface(display, pbuffer), TRUE);
        assert_eq!(guard.with(|s| (s.confirmed, s.materialized)), (9, 2));
        assert_eq!(guard.with(|s| s.aliases[&(identity(old), original)]), 1);
        let recycled = state(logical).generation;
        assert_eq!(guard.with(|s| s.aliases[&(identity(logical), recycled)]), 6);
    }
}
