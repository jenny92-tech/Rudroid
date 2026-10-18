//! INPUT:  synthetic CPU text and the bounded sealed resource view
//! OUTPUT:  exact namespace, independent cursor and kernel mutation refusal checks
//! POS:    verifies process/file ownership without executing Unity
#![forbid(unsafe_code)]
use super::*;
use std::{io::{Read, Write}, os::unix::ffi::OsStrExt};

#[test]
fn exact_readers_have_independent_cursors_and_remain_live_after_owner_drops() {
    let view=CpuInfoView::from_bytes(b"processor: 0\nfeatures: fp asimd\n").unwrap();
    assert!(view.redirect(c"/proc/meminfo").is_none());
    assert!(view.redirect(c"/proc/cpuinfo/other").is_none());
    let path=view.redirect(c"/proc/cpuinfo").unwrap();
    let path=std::path::Path::new(std::ffi::OsStr::from_bytes(CStr::from_bytes_with_nul(&path).unwrap().to_bytes()));
    let mut a=File::open(path).unwrap();
    let mut b=File::open(path).unwrap();
    a.read_exact(&mut [0;3]).unwrap();
    let mut text=String::new();b.read_to_string(&mut text).unwrap();
    assert!(text.starts_with("processor: 0"));
    let mut writer=std::fs::OpenOptions::new().write(true).open(path).unwrap();
    assert!(writer.write_all(b"overwrite").is_err());
    assert!(writer.set_len(0).is_err());
    drop(view);
    let mut rest=String::new();a.read_to_string(&mut rest).unwrap();
    assert!(rest.starts_with("cessor: 0"));
}
