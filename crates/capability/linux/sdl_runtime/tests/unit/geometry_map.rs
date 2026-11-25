//! Real SDL/GLES regression run from the existing single video-owner test.

use super::*;
use crate::window::create_session_sized;
use rudroid_common::registry::HostStr;

pub(crate) fn exercise() {
    extern "C" fn enable(key: HostStr<'_>, fallback: i32) -> i32 {
        if key.to_cstr() == Some(c"gpu.glesWholeBufferInvalidate") {
            1
        } else {
            fallback
        }
    }

    for enabled in [false, true] {
        if enabled {
            let mut api = HostApiV0::stubbed();
            api.config_get_bool = enable;
            assert!(configure_with_diagnostics(&api, true));
        }
        let (mut session, _) = create_session_sized(64, 64, 3, 0).unwrap();
        // SAFETY: this test owns the current context and every native object
        // and pointer; all entry points have their exact GLES prototypes.
        unsafe {
            session.activate_guest_graphics(None).unwrap();
            let g = active().unwrap();
            let surface = g.surface as *mut c_void;
            assert_eq!(super::super::make_current(g.display as _, surface, surface, g.context as _), 1);
            assert_eq!(g.geometry_maps.is_some(), enabled);

            macro_rules! resolve {
                ($name:literal, $ty:ty) => {{
                    let addr = g.resolve($name);
                    assert_ne!(addr, 0);
                    std::mem::transmute::<usize, $ty>(addr)
                }};
            }
            let generate = resolve!(c"glGenBuffers", extern "C" fn(i32, *mut u32));
            let delete = resolve!(c"glDeleteBuffers", extern "C" fn(i32, *const u32));
            let bind = resolve!(c"glBindBuffer", extern "C" fn(u32, u32));
            let data = resolve!(c"glBufferData", extern "C" fn(u32, isize, *const c_void, u32));
            let map = resolve!(c"glMapBufferRange", Map);
            let flush = resolve!(c"glFlushMappedBufferRange", extern "C" fn(u32, isize, isize));
            let unmap = resolve!(c"glUnmapBuffer", Unmap);
            let get = resolve!(c"glGetBufferParameteriv", Query32);
            let get64 = resolve!(c"glGetBufferParameteri64v", Query64);
            let attrib = resolve!(c"glVertexAttribPointer", extern "C" fn(u32, i32, u32, u8, i32, *const c_void));
            let get_attrib = resolve!(c"glGetVertexAttribiv", extern "C" fn(u32, u32, *mut i32));
            let get_error = resolve!(c"glGetError", extern "C" fn() -> u32);
            let native_get: Query32 = std::mem::transmute(g.native_address(c"glGetBufferParameteriv"));
            assert_eq!(g.resolve(c"glMapBufferRange") == g.native_address(c"glMapBufferRange"), !enabled);
            assert_eq!(g.resolve(c"glFlushMappedBufferRange"), g.native_address(c"glFlushMappedBufferRange"));

            let mut names = [0; 4];
            generate(4, names.as_mut_ptr());
            bind(ARRAY, names[0]);
            data(ARRAY, 32, std::ptr::null(), DYNAMIC_DRAW as u32);
            attrib(0, 2, 0x1406, 0, 8, std::ptr::null());
            bind(COPY_WRITE, names[0]);
            let bytes = map(COPY_WRITE, 0, 32, RANGE).cast::<u8>();
            assert!(!bytes.is_null());
            bytes.write_bytes(11, 32);
            flush(COPY_WRITE, 0, 32);
            let mut flags = 0;
            native_get(ARRAY, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, if enabled { WHOLE } else { RANGE } as i32);
            get(ARRAY, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, RANGE as i32);
            let mut wide = 0;
            get64(COPY_WRITE, ACCESS_FLAGS, &mut wide);
            assert_eq!(wide, i64::from(RANGE));

            // Overlapping independent buffers keep their distinct identities.
            bind(COPY_WRITE, names[1]);
            data(COPY_WRITE, 32, std::ptr::null(), STREAM_DRAW as u32);
            assert!(!map(COPY_WRITE, 0, 32, RANGE).is_null());
            native_get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, if enabled { WHOLE } else { RANGE } as i32);
            get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, RANGE as i32);
            assert_eq!(unmap(COPY_WRITE), 1);
            // A guest that already asked for whole invalidation sees its flags.
            assert!(!map(COPY_WRITE, 0, 32, WHOLE).is_null());
            get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, WHOLE as i32);
            assert_eq!(unmap(COPY_WRITE), 1);
            get(ARRAY, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, RANGE as i32);
            assert_eq!(unmap(ARRAY), 1);

            // Original content and VAO binding survive a later partial map
            // through the copy target, including its relative flush offsets.
            bind(COPY_WRITE, names[0]);
            let bytes = map(COPY_WRITE, 4, 8, RANGE).cast::<u8>();
            assert!(!bytes.is_null());
            native_get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, RANGE as i32);
            bytes.write_bytes(73, 8);
            flush(COPY_WRITE, 0, 8);
            assert_eq!(unmap(COPY_WRITE), 1);
            let bytes = map(ARRAY, 0, 32, 1).cast::<u8>();
            assert!(!bytes.is_null());
            let mut expected = [11u8; 32];
            expected[4..12].fill(73);
            assert_eq!(std::slice::from_raw_parts(bytes, 32), expected);
            assert_eq!(unmap(ARRAY), 1);
            let mut attribute = 0;
            get_attrib(0, 0x889f, &mut attribute);
            assert_eq!(attribute as u32, names[0]);
            assert_eq!(get_error(), 0);
            assert!(map(COPY_WRITE, 0, 33, RANGE).is_null());
            assert_eq!(get_error(), 0x0501);
            bind(COPY_WRITE, 0);
            assert!(map(COPY_WRITE, 0, 32, RANGE).is_null());
            assert_eq!(get_error(), 0x0502);

            let assert_native_flags = |target, expected| {
                let mut actual = -1;
                native_get(target, ACCESS_FLAGS, &mut actual);
                assert_eq!(actual, expected as i32);
            };
            let whole_hint = if enabled { WHOLE } else { RANGE };
            let bytes = map(ARRAY, 0, 32, RANGE).cast::<u8>();
            assert!(!bytes.is_null());
            bytes.write_bytes(19, 32);
            flush(ARRAY, 0, 32);
            assert_native_flags(ARRAY, whole_hint);
            bind(COPY_WRITE, names[0]);
            assert_native_flags(COPY_WRITE, whole_hint);
            get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
            assert_eq!(flags, RANGE as i32);
            assert_eq!(unmap(COPY_WRITE), 1);

            let generate_vao = resolve!(c"glGenVertexArrays", extern "C" fn(i32, *mut u32));
            let bind_vao = resolve!(c"glBindVertexArray", extern "C" fn(u32));
            let delete_vao = resolve!(c"glDeleteVertexArrays", extern "C" fn(i32, *const u32));
            let mut vaos = [0; 2];
            generate_vao(2, vaos.as_mut_ptr());
            for (vao, name, len) in [(vaos[0], names[2], 32), (vaos[1], names[3], 64)] {
                let original = [37u8; 64];
                bind_vao(vao);
                bind(ELEMENT_ARRAY, name);
                data(ELEMENT_ARRAY, len, original.as_ptr().cast(), STREAM_DRAW as u32);
            }
            bind(COPY_WRITE, names[2]);
            for (vao, hint) in [(vaos[0], whole_hint), (vaos[1], RANGE)] {
                bind_vao(vao);
                let bytes = map(ELEMENT_ARRAY, 0, 32, RANGE).cast::<u8>();
                assert!(!bytes.is_null());
                assert_native_flags(ELEMENT_ARRAY, hint);
                get(ELEMENT_ARRAY, ACCESS_FLAGS, &mut flags);
                assert_eq!(flags, RANGE as i32);
                if vao == vaos[0] {
                    // The mapping still belongs to the VAO's original object
                    // when it is queried through another binding target.
                    assert_native_flags(COPY_WRITE, hint);
                    get(COPY_WRITE, ACCESS_FLAGS, &mut flags);
                    assert_eq!(flags, RANGE as i32);
                }
                bytes.write_bytes(61, 32);
                flush(ELEMENT_ARRAY, 0, 32);
                assert_eq!(unmap(ELEMENT_ARRAY), 1);
            }
            let bytes = map(ELEMENT_ARRAY, 0, 64, 1).cast::<u8>();
            assert!(!bytes.is_null());
            assert_eq!(std::slice::from_raw_parts(bytes.add(32), 32), [37; 32]);
            assert_eq!(std::slice::from_raw_parts(bytes, 32), [61; 32]);
            assert_eq!(unmap(ELEMENT_ARRAY), 1);
            bind_vao(0);
            delete_vao(2, vaos.as_ptr());

            // A shared worker context sees the same object but retains its
            // original hint. Only the SDL primary window owner may translate.
            use super::super::{create_context, create_pbuffer, destroy_context, destroy_surface, make_current, NONE};
            let worker = create_context(g.display as _, g.config as _, g.context as _, [0x3098, 3, NONE].as_ptr());
            let pbuffer = create_pbuffer(g.display as _, g.config as _, [0x3057, 4, 0x3056, 4, NONE].as_ptr());
            assert!(!worker.is_null() && !pbuffer.is_null());
            assert_eq!(make_current(g.display as _, pbuffer, pbuffer, worker), 1);
            bind(ARRAY, names[0]);
            let bytes = map(ARRAY, 0, 32, RANGE).cast::<u8>();
            assert!(!bytes.is_null());
            assert_native_flags(ARRAY, RANGE);
            bytes.write_bytes(19, 32);
            flush(ARRAY, 0, 32);
            assert_eq!(unmap(ARRAY), 1);
            assert_eq!(make_current(g.display as _, surface, surface, g.context as _), 1);
            assert_eq!(destroy_context(g.display as _, worker), 1);
            assert_eq!(destroy_surface(g.display as _, pbuffer), 1);
            delete(4, names.as_ptr());
            assert_eq!(get_error(), 0);
        }
    }
    println!("[GLES-WHOLE-MAP] real GLES off/on, COPY_WRITE, ARRAY, distinct VAOs, partial bytes and worker fallback verified");
}
