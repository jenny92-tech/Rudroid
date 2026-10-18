//! INPUT:  safe GPU policy and simulated native extension inventories
//! OUTPUT:  coherent disabled-binary, indexed-extension, replacement and scalar-limit cases
//! POS:    compatibility policy regressions independent of a driver or native pointers
use super::*;
fn policy() -> Policy {
    Policy {
        binary_disabled: false,
        cache_strings: true,
        sharp_half_precision: false,
        limits: vec![],
        replace: vec![],
        append: vec![],
        remove: vec![],
    }
}
#[test]
fn binary_disable_removes_advertisement_and_both_format_queries() {
    let mut p = policy();
    p.binary_disabled = true;
    p.append = vec![BINARY_EXTENSIONS[0].into(), "GL_EXT_keep".into()];
    let result = p
        .extensions(&[
            BINARY_EXTENSIONS[0].into(),
            "GL_EXT_keep".into(),
            BINARY_EXTENSIONS[2].into(),
        ])
        .unwrap();
    assert_eq!(result.joined.as_c_str(), c"GL_EXT_keep");
    assert_eq!(result.words.len(), 1);
    assert_eq!(p.integer(0x87fe), Some(0));
    assert_eq!(p.integer(0x87ff), Some(0));
    assert_eq!(policy().integer(0x87fe), None);
}
#[test]
fn extension_replace_remove_append_preserve_order_and_index_identity() {
    let mut p = policy();
    p.replace = vec!["GL_A".into(), "GL_B".into()];
    p.remove = vec!["GL_A".into()];
    p.append = vec!["GL_C".into(), "GL_B".into()];
    let e = p.extensions(&["GL_NATIVE".into()]).unwrap();
    assert_eq!(e.joined.as_c_str(), c"GL_B GL_C");
    assert_eq!(e.words[0].as_c_str(), c"GL_B");
    assert_eq!(e.words[1].as_c_str(), c"GL_C");
    assert!(e.words.get(2).is_none());
    assert!(words("GL_A bad-token".into()).is_err());
}
#[test]
fn scalar_limits_do_not_shadow_vector_state_queries() {
    let mut p = policy();
    p.limits = vec![(0x0d33, 1024), (0x8d57, 0)];
    assert_eq!(p.integer(0x0d33), Some(1024));
    assert_eq!(p.integer(0x8d57), Some(0));
    assert_eq!(p.integer(0x0ba2), None);
    assert_eq!(p.integer(0x87ff), None);
}

#[test]
fn fixed_string_cache_pins_one_copy_per_valid_query() {
    let cache = StringCache::new();
    for name in [0x1f00, 0x1f01, 0x1f02, 0x1f03, 0x8b8c] {
        assert!(cache.get(name).is_none());
        let original = CString::new(format!("driver-{name}")).unwrap();
        let pointer = cache.remember(name, &original).unwrap().as_ptr();
        drop(original);
        assert_eq!(
            cache.remember(name, c"replacement").unwrap().as_ptr(),
            pointer
        );
        assert_eq!(
            cache.get(name).unwrap().to_bytes(),
            format!("driver-{name}").as_bytes()
        );
    }
    assert!(cache.remember(0, c"invalid").is_none());
    assert!(cache.get(u32::MAX).is_none());
}

#[test]
fn concurrent_string_misses_publish_one_stable_owned_pointer() {
    let cache = StringCache::new();
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    let pointer =
                        cache.remember(0x1f02, c"OpenGL ES 3.2").unwrap().as_ptr() as usize;
                    for _ in 0..10000 {
                        assert_eq!(cache.get(0x1f02).unwrap().as_ptr() as usize, pointer);
                        assert_eq!(
                            cache.remember(0x1f02, c"ignored").unwrap().as_ptr() as usize,
                            pointer
                        );
                    }
                    pointer
                })
            })
            .collect();
        let expected = cache.get(0x1f02).map(|s| s.as_ptr() as usize);
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(results.iter().all(|p| *p == results[0]));
        if let Some(p) = expected {
            assert_eq!(p, results[0]);
        }
    });
}
