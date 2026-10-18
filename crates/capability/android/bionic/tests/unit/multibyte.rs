//! INPUT:  private items of `src/multibyte.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/multibyte.rs`; declared there with `#[path]`
use super::*;
#[test]
fn all_unicode_scalars_round_trip_with_every_split_and_unaligned_guest_state() {
    unsafe {
        for scalar in 0..=0x10ffff {
            let Some(character) = char::from_u32(scalar) else {
                continue;
            };
            let mut oracle = [0; 4];
            let expected = character.encode_utf8(&mut oracle).as_bytes();
            let mut encoded = [0; 6];
            encoded[0] = 73;
            encoded[5] = 84;
            assert_eq!(
                encode(encoded.as_mut_ptr().add(1), scalar, &mut [0; 4]),
                expected.len()
            );
            assert_eq!(&encoded[1..1 + expected.len()], expected);
            assert_eq!((encoded[0], encoded[5]), (73, 84));
            for split in 0..expected.len() {
                let mut storage = [0xa5; 10];
                storage[1..5].fill(0);
                let ps = storage.as_mut_ptr().add(1);
                let mut output = u32::MAX;
                assert_eq!(
                    mbrtowc(&mut output, expected.as_ptr().cast(), split, ps),
                    INCOMPLETE
                );
                assert_eq!(output, u32::MAX);
                let result = mbrtowc(
                    &mut output,
                    expected.as_ptr().add(split).cast(),
                    expected.len() - split,
                    ps,
                );
                assert_eq!(
                    result,
                    if scalar == 0 {
                        0
                    } else {
                        expected.len() - split
                    }
                );
                assert_eq!(output, scalar);
                assert_eq!(mbsinit(ps), 1);
                assert_eq!(storage[0], 0xa5);
                assert_eq!(&storage[5..], &[0xa5; 5]);
            }
        }
    }
}
#[test]
fn illegal_decode_resets_and_legacy_encoder_range_matches_android() {
    unsafe {
        for bytes in [
            &[0xc0, 0x80][..],
            &[0xed, 0xa0, 0x80],
            &[0xf4, 0x90, 0x80, 0x80],
            &[0x80],
            &[0xe2, 0x20],
        ] {
            let mut state = [0; 4];
            assert_eq!(
                decode(ptr::null_mut(), bytes.as_ptr(), bytes.len(), &mut state),
                ERROR
            );
            assert_eq!(*libc::__errno_location(), libc::EILSEQ);
            assert_eq!(state, [0; 4]);
            let mut value = 0;
            assert_eq!(decode(&mut value, b"Z".as_ptr(), 1, &mut state), 1);
            assert_eq!(value, 90);
        }
        let mut state = [0xf0, 0, 0, 0];
        assert_eq!(encode(ptr::null_mut(), 0xffffffff, &mut state), 1);
        assert_eq!(state, [0; 4]);
        let mut bytes = [0; 4];
        assert_eq!(encode(bytes.as_mut_ptr(), 0xd800, &mut state), 3);
        assert_eq!(&bytes[..3], &[0xed, 0xa0, 0x80]);
        assert_eq!(encode(bytes.as_mut_ptr(), 0x200000, &mut state), ERROR);
    }
}
#[test]
fn private_restart_state_is_thread_local() {
    let threads: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| unsafe {
                for _ in 0..1000 {
                    let mut out = 0;
                    assert_eq!(
                        mbrtowc(&mut out, c"\xe4".as_ptr(), 1, ptr::null_mut()),
                        INCOMPLETE
                    );
                    std::thread::yield_now();
                    assert_eq!(
                        mbrtowc(&mut out, c"\xb8\xad".as_ptr(), 2, ptr::null_mut()),
                        2
                    );
                    assert_eq!(out, 0x4e2d);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
