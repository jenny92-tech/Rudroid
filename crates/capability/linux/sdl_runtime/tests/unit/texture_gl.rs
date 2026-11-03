//! INPUT:  real handheld Wayland/Mali, padded CPU pixels and native PBOs
//! OUTPUT:  real pixel readback, storage/mip, error and shared-context ownership evidence
//! POS:    opt-in native acceptance of the production texture facade
use super::*;
#[test]
#[ignore = "run alone on the actual handheld with Wayland/Mali; creates a fullscreen GL session"]
fn handheld_texture_caps_preserve_pixels_and_native_state() {
    use rudroid_common::{
        host_api::{ConfigGetStringFn, HostApiV0},
        registry::HostStr,
    };
    extern "C" fn integer(key: HostStr<'_>, default: i64) -> i64 {
        match key.to_cstr() {
            Some(k)
                if [
                    c"gpu.textureMaxDimRGBA8",
                    c"gpu.textureMaxDimETC2",
                    c"gpu.textureMaxDimASTC",
                ]
                .contains(&k) =>
            {
                64
            }
            _ => default,
        }
    }
    extern "C" fn boolean(key: HostStr<'_>, default: i32) -> i32 {
        if key.to_cstr() == Some(c"gpu.textureCapsStats") {
            1
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
        let value = if std::env::var("RUDROID_TEST_TEXTURE_FILTER").as_deref() == Ok("nearest") {
            c"nearest"
        } else {
            c"box"
        };
        let bytes = value.to_bytes_with_nul();
        if size as usize >= bytes.len() && !out.is_null() {
            // SAFETY: fixture Host API supplies size writable bytes; source is a static C string.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), out, bytes.len());
            }
        }
        1
    }
    let mut api = HostApiV0::stubbed();
    api.config_get_i64 = integer;
    api.config_get_bool = boolean;
    // SAFETY: fixture callback bounds every write and remains process-pinned.
    api.config_get_string = unsafe { ConfigGetStringFn::new(string) };
    super::super::texture_policy::configure(&api).unwrap();
    let (mut session, _) = crate::window::create_session_sized(640, 480, 3, 0).unwrap();
    // SAFETY: this test exclusively owns the native context and all passed memory. Typed
    // pointers use exact GLES prototypes; CPU/PBO lengths and readback pack state are explicit.
    unsafe {
        session.activate_guest_graphics(None).unwrap();
        let g = active().unwrap();
        let display = g.display as Handle;
        let surface = g.surface as Handle;
        let primary = g.context as Handle;
        assert_eq!(
            create_context(
                display,
                g.config as Handle,
                std::ptr::null_mut(),
                [NONE].as_ptr()
            ),
            primary
        );
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        macro_rules! gl {
            ($name:literal,$ty:ty) => {{
                let a = g.resolve($name);
                assert_ne!(a, 0);
                std::mem::transmute::<usize, $ty>(a)
            }};
        }
        let error = gl!(c"glGetError", extern "C" fn() -> u32);
        let gen = gl!(c"glGenTextures", extern "C" fn(i32, *mut u32));
        let bind = gl!(c"glBindTexture", extern "C" fn(u32, u32));
        let image = gl!(
            c"glTexImage2D",
            extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void)
        );
        let sub = gl!(
            c"glTexSubImage2D",
            extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void)
        );
        let storage = gl!(c"glTexStorage2D", extern "C" fn(u32, i32, u32, i32, i32));
        let compressed = gl!(
            c"glCompressedTexImage2D",
            extern "C" fn(u32, i32, u32, i32, i32, i32, i32, *const c_void)
        );
        let compressed_sub = gl!(
            c"glCompressedTexSubImage2D",
            extern "C" fn(u32, i32, i32, i32, i32, i32, u32, i32, *const c_void)
        );
        let parameter_i = gl!(c"glTexParameteri", extern "C" fn(u32, u32, i32));
        let parameter_fv = gl!(c"glTexParameterfv", extern "C" fn(u32, u32, *const f32));
        let query_i = gl!(c"glGetTexParameteriv", extern "C" fn(u32, u32, *mut i32));
        let query_f = gl!(c"glGetTexParameterfv", extern "C" fn(u32, u32, *mut f32));
        let native_query_i = std::mem::transmute::<usize, extern "C" fn(u32, u32, *mut i32)>(
            g.native_address(c"glGetTexParameteriv"),
        );
        let delete = gl!(c"glDeleteTextures", extern "C" fn(i32, *const u32));
        let pack = gl!(c"glPixelStorei", extern "C" fn(u32, i32));
        let get = gl!(c"glGetIntegerv", extern "C" fn(u32, *mut i32));
        let level = gl!(
            c"glGetTexLevelParameteriv",
            extern "C" fn(u32, i32, u32, *mut i32)
        );
        let gen_fbo = gl!(c"glGenFramebuffers", extern "C" fn(i32, *mut u32));
        let bind_fbo = gl!(c"glBindFramebuffer", extern "C" fn(u32, u32));
        let attach = gl!(
            c"glFramebufferTexture2D",
            extern "C" fn(u32, u32, u32, u32, i32)
        );
        let status = gl!(c"glCheckFramebufferStatus", extern "C" fn(u32) -> u32);
        let read = gl!(
            c"glReadPixels",
            extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void)
        );
        let gen_buffer = gl!(c"glGenBuffers", extern "C" fn(i32, *mut u32));
        let bind_buffer = gl!(c"glBindBuffer", extern "C" fn(u32, u32));
        let buffer_data = gl!(
            c"glBufferData",
            extern "C" fn(u32, isize, *const c_void, u32)
        );
        let map = gl!(
            c"glMapBufferRange",
            extern "C" fn(u32, isize, isize, u32) -> *const c_void
        );
        let unmap = gl!(c"glUnmapBuffer", extern "C" fn(u32) -> u8);
        let query = |key| {
            let mut v = 0;
            get(key, &mut v);
            v
        };
        let width = || {
            let mut v = 0;
            level(0x0de1, 0, 0x1000, &mut v);
            v
        };
        let mut names = [0; 10];
        gen(10, names.as_mut_ptr());
        let mut fbo = 0;
        gen_fbo(1, &mut fbo);
        let pixel = |texture| {
            bind_fbo(0x8d40, fbo);
            attach(0x8d40, 0x8ce0, 0x0de1, texture, 0);
            assert_eq!(status(0x8d40), 0x8cd5);
            pack(0x0d05, 1);
            for k in [0x0d02, 0x0d03, 0x0d04] {
                pack(k, 0);
            }
            let mut rgba = [0u8; 4];
            read(0, 0, 1, 1, 0x1908, 0x1401, rgba.as_mut_ptr().cast());
            assert_eq!(error(), 0);
            rgba
        };
        let row = 132usize;
        let stride = row * 4;
        let mut pixels = vec![221u8; stride * 98];
        for y in 0..96 {
            for x in 0..128 {
                let i = (y + 1) * stride + (x + 2) * 4;
                pixels[i..i + 4].copy_from_slice(&[(x * 2) as u8, (y * 2) as u8, 173, 255]);
            }
        }
        pack(0x0cf5, 8);
        pack(0x0cf2, row as i32);
        pack(0x0cf3, 1);
        pack(0x0cf4, 2);
        bind(0x0de1, names[0]);
        image(
            0x0de1,
            0,
            0x8058,
            128,
            96,
            0,
            0x1908,
            0x1401,
            pixels.as_ptr().cast(),
        );
        assert_eq!(error(), 0);
        assert_eq!(width(), 64);
        assert_eq!(
            [query(0x0cf5), query(0x0cf2), query(0x0cf3), query(0x0cf4)],
            [8, 132, 1, 2]
        );
        let first = if std::env::var("RUDROID_TEST_TEXTURE_FILTER").as_deref() == Ok("nearest") {
            0
        } else {
            1
        };
        assert_eq!(pixel(names[0]), [first, first, 173, 255]);
        // Invalid redefinition and pre-existing errors remain visible and cannot poison metadata.
        pack(0xffff, 0);
        image(
            0x0de1,
            0,
            0x8058,
            -1,
            96,
            0,
            0x1908,
            0x1401,
            std::ptr::null(),
        );
        let a = error();
        let b = error();
        assert_eq!([a, b], [0x0500, 0x0501]);
        assert_eq!(error(), 0);
        assert_eq!(width(), 64);
        // Immutable allocation shrinks once; a rejected second allocation keeps the first plan.
        bind(0x0de1, names[1]);
        storage(0x0de1, 8, 0x8058, 128, 96);
        assert_eq!(error(), 0);
        assert_eq!(width(), 64);
        storage(0x0de1, 1, 0x8058, 16, 16);
        assert_eq!(error(), 0x0502);
        assert_eq!(width(), 64);
        let mut buffer = 0;
        gen_buffer(1, &mut buffer);
        bind_buffer(0x88ec, buffer);
        let mut contents = vec![99u8; 64];
        contents.extend_from_slice(&pixels);
        buffer_data(
            0x88ec,
            contents.len() as isize,
            contents.as_ptr().cast(),
            0x88e4,
        );
        sub(
            0x0de1,
            0,
            0,
            0,
            128,
            96,
            0x1908,
            0x1401,
            64usize as *const c_void,
        );
        assert_eq!(error(), 0);
        assert_eq!(query(0x88ef), buffer as i32);
        assert_eq!(query(0x0cf2), 132);
        assert_eq!(pixel(names[1]), [first, first, 173, 255]);
        let p = map(0x88ec, 0, contents.len() as isize, 1);
        assert!(!p.is_null());
        assert_eq!(
            std::slice::from_raw_parts(p.cast::<u8>(), contents.len()),
            contents
        );
        sub(
            0x0de1,
            0,
            0,
            0,
            32,
            24,
            0x1908,
            0x1401,
            64usize as *const c_void,
        );
        assert_eq!(error(), 0x0502);
        assert_eq!(unmap(0x88ec), 1);
        // A NULL pointer with a PBO is offset zero, rather than an uninitialized image.
        buffer_data(
            0x88ec,
            pixels.len() as isize,
            pixels.as_ptr().cast(),
            0x88e4,
        );
        bind(0x0de1, names[7]);
        image(
            0x0de1,
            0,
            0x8058,
            128,
            96,
            0,
            0x1908,
            0x1401,
            std::ptr::null(),
        );
        assert_eq!(error(), 0);
        assert_eq!(width(), 64);
        assert_eq!(pixel(names[7]), [first, first, 173, 255]);
        bind_buffer(0x88ec, 0);
        // Exact render-sized and narrow resources stay at their original dimensions.
        bind(0x0de1, names[2]);
        storage(0x0de1, 1, 0x8058, 128, 16);
        assert_eq!(width(), 128);
        bind(0x0de1, names[3]);
        storage(0x0de1, 1, 0x8058, g.width, g.height);
        assert_eq!(width(), g.width);
        assert_eq!(error(), 0);
        // ETC2 known mip chains rebase, while direct/no-mip compressed allocations stay intact.
        bind(0x0de1, names[4]);
        storage(0x0de1, 8, 0x9274, 128, 128);
        assert_eq!(width(), 64);
        assert_eq!(error(), 0);
        let zero = vec![0u8; 8192];
        compressed_sub(
            0x0de1,
            0,
            0,
            0,
            128,
            128,
            0x9274,
            8192,
            zero.as_ptr().cast(),
        );
        assert_eq!(error(), 0);
        compressed_sub(0x0de1, 1, 0, 0, 64, 64, 0x9274, 2048, zero.as_ptr().cast());
        assert_eq!(error(), 0);
        bind(0x0de1, names[5]);
        storage(0x0de1, 1, 0x9274, 128, 128);
        assert_eq!(width(), 128);
        assert_eq!(error(), 0);
        bind(0x0de1, names[6]);
        compressed(0x0de1, 0, 0x9274, 128, 128, 0, 8192, zero.as_ptr().cast());
        assert_eq!(width(), 128);
        assert_eq!(error(), 0);
        bind(0x0de1, names[4]);
        parameter_i(0x0de1, 0x813c, 1);
        parameter_i(0x0de1, 0x813d, 16777217);
        let mut exact = [0, 77];
        query_i(0x0de1, 0x813d, exact.as_mut_ptr());
        assert_eq!(exact, [16777217, 77]);
        let mut v = 0;
        native_query_i(0x0de1, 0x813c, &mut v);
        assert_eq!(v, 0);
        let lod = [1.75f32];
        parameter_fv(0x0de1, 0x813a, lod.as_ptr());
        let mut value = [0.0, 88.0];
        query_f(0x0de1, 0x813a, value.as_mut_ptr());
        assert_eq!(value, [1.75, 88.0]);
        parameter_i(0x0de1, 0x813c, -1);
        assert_eq!(error(), 0x0501);
        query_i(0x0de1, 0x813c, &mut v);
        assert_eq!(v, 1);
        // Real ASTC allocation/upload, plus sampling both compressed families through a native program.
        bind(0x0de1, names[8]);
        storage(0x0de1, 8, 0x93b0, 128, 128);
        assert_eq!(width(), 64);
        assert_eq!(error(), 0);
        let white: [u8; 16] = [
            0xfc, 0xfd, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ];
        let astc = white.repeat(256);
        compressed_sub(0x0de1, 1, 0, 0, 64, 64, 0x93b0, 4096, astc.as_ptr().cast());
        assert_eq!(error(), 0);
        for k in [0x0cf2, 0x0cf3, 0x0cf4] {
            pack(k, 0);
        }
        pack(0x0cf5, 1);
        bind(0x0de1, names[9]);
        image(0x0de1, 0, 0x8058, 4, 4, 0, 0x1908, 0x1401, std::ptr::null());
        assert_eq!(error(), 0);
        let create_shader = gl!(c"glCreateShader", extern "C" fn(u32) -> u32);
        let source = gl!(
            c"glShaderSource",
            extern "C" fn(u32, i32, *const *const c_char, *const i32)
        );
        let compile = gl!(c"glCompileShader", extern "C" fn(u32));
        let shader_query = gl!(c"glGetShaderiv", extern "C" fn(u32, u32, *mut i32));
        let create_program = gl!(c"glCreateProgram", extern "C" fn() -> u32);
        let attach_shader = gl!(c"glAttachShader", extern "C" fn(u32, u32));
        let link = gl!(c"glLinkProgram", extern "C" fn(u32));
        let program_query = gl!(c"glGetProgramiv", extern "C" fn(u32, u32, *mut i32));
        let use_program = gl!(c"glUseProgram", extern "C" fn(u32));
        let viewport = gl!(c"glViewport", extern "C" fn(i32, i32, i32, i32));
        let draw = gl!(c"glDrawArrays", extern "C" fn(u32, i32, i32));
        let program = create_program();
        for (kind,text) in [(0x8b31,c"#version 300 es\nout vec2 uv;void main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);uv=p;gl_Position=vec4(p*2.0-1.0,0,1);}"),
            (0x8b30,c"#version 300 es\nprecision mediump float;in vec2 uv;uniform sampler2D tex;out vec4 color;void main(){color=texture(tex,uv);}")] {
            let shader=create_shader(kind);let pointer=text.as_ptr();source(shader,1,&pointer,std::ptr::null());compile(shader);let mut good=0;shader_query(shader,0x8b81,&mut good);assert_eq!(good,1);attach_shader(program,shader);
        }
        link(program);
        let mut good = 0;
        program_query(program, 0x8b82, &mut good);
        assert_eq!(good, 1);
        use_program(program);
        for (texture, expected) in [(names[4], [2, 2, 2, 255]), (names[8], [255; 4])] {
            bind(0x0de1, texture);
            parameter_i(0x0de1, 0x813c, 1);
            parameter_i(0x0de1, 0x813d, 1);
            for (key, value) in [(0x813a, 0.0), (0x813b, 1000.0)] {
                parameter_fv(0x0de1, key, &value);
            }
            parameter_i(0x0de1, 0x2801, 0x2600);
            parameter_i(0x0de1, 0x2800, 0x2600);
            bind_fbo(0x8d40, fbo);
            attach(0x8d40, 0x8ce0, 0x0de1, names[9], 0);
            viewport(0, 0, 4, 4);
            draw(4, 0, 3);
            assert_eq!(
                pixel(names[9]),
                expected,
                "compressed texture must actually sample its retained original mip"
            );
        }
        // Shared worker metadata survives destroy while current, then native unbind retires it.
        let shared = create_context(
            display,
            g.config as Handle,
            primary,
            [0x3098, 3, NONE].as_ptr(),
        );
        assert!(!shared.is_null());
        assert_eq!(make_current(display, surface, surface, shared), TRUE);
        bind(0x0de1, names[1]);
        assert_eq!(destroy_context(display, shared), TRUE);
        for k in [0x0cf2, 0x0cf3, 0x0cf4] {
            pack(k, 0);
        }
        pack(0x0cf5, 1);
        let red = vec![254u8; 128 * 96 * 4];
        sub(
            0x0de1,
            0,
            0,
            0,
            128,
            96,
            0x1908,
            0x1401,
            red.as_ptr().cast(),
        );
        assert_eq!(error(), 0);
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        assert_eq!(pixel(names[1]), [254; 4]);
        // A new independent context can have identical integer texture names without inheriting a cap.
        let independent = create_context(
            display,
            g.config as Handle,
            std::ptr::null_mut(),
            [0x3098, 3, NONE].as_ptr(),
        );
        assert!(!independent.is_null());
        assert_eq!(make_current(display, surface, surface, independent), TRUE);
        let mut other = 0;
        gen(1, &mut other);
        bind(0x0de1, other);
        let green = vec![72u8; 16 * 16 * 4];
        image(
            0x0de1,
            0,
            0x8058,
            16,
            16,
            0,
            0x1908,
            0x1401,
            green.as_ptr().cast(),
        );
        assert_eq!(width(), 16);
        assert_eq!(error(), 0);
        delete(1, &other);
        assert_eq!(error(), 0);
        assert_eq!(make_current(display, surface, surface, primary), TRUE);
        assert_eq!(destroy_context(display, independent), TRUE);
        delete(10, names.as_ptr());
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
        drop(g);
        drop(session);
    }
}
