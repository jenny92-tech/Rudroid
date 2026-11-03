//! INPUT:  synthetic ELF headers/notes and borrowed temporary files
//! OUTPUT:  bounded identity parsing, cursor stability and malformed-input checks
//! POS:    file preflight regression; fixtures contain no executable native code
use super::*;
use std::{
    io::{Seek, SeekFrom, Write},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn image(id: &[u8]) -> Vec<u8> {
    let mut b = vec![0; 128 + 16 + (id.len() + 3) / 4 * 4];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&3u16.to_le_bytes());
    b[18..20].copy_from_slice(&183u16.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&4u32.to_le_bytes());
    b[72..80].copy_from_slice(&128u64.to_le_bytes());
    let n = b.len() as u64 - 128;
    b[96..104].copy_from_slice(&n.to_le_bytes());
    b[128..132].copy_from_slice(&4u32.to_le_bytes());
    b[132..136].copy_from_slice(&(id.len() as u32).to_le_bytes());
    b[136..140].copy_from_slice(&3u32.to_le_bytes());
    b[140..144].copy_from_slice(b"GNU\0");
    b[144..144 + id.len()].copy_from_slice(id);
    b
}
fn inspect(bytes: &[u8]) -> io::Result<ElfIdentity> {
    let path = std::env::temp_dir().join(format!(
        "rudroid-elf-id-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(path).unwrap();
    file.write_all(bytes).unwrap();
    file.seek(SeekFrom::Start(3)).unwrap();
    let result = aarch64_identity(&file);
    assert_eq!(file.stream_position().unwrap(), 3);
    result
}
#[test]
fn gnu_note_is_read_without_changing_file_cursor() {
    let b = image(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let id = inspect(&b).unwrap();
    assert_eq!(id.build_id, Some(vec![1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(id.file_len, b.len() as u64);
}
#[test]
fn unknown_architecture_truncation_and_oversized_ranges_reject() {
    let original = image(&[1; 8]);
    for index in [4, 5, 18, 54, 56] {
        let mut b = original.clone();
        b[index] = 0;
        assert!(inspect(&b).is_err(), "header byte {index}");
    }
    for n in [0, 63, 119, 127, 151] {
        assert!(inspect(&original[..n]).is_err());
    }
    let mut b = original.clone();
    b[96..104].copy_from_slice(&65537u64.to_le_bytes());
    assert!(inspect(&b).is_err());
    let mut b = original.clone();
    b[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(inspect(&b).is_err());
    let mut b = original.clone();
    b[128..132].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(inspect(&b).is_err());
}
#[test]
fn duplicate_build_notes_reject_and_absent_id_is_distinct() {
    let mut b = image(&[7; 8]);
    let second = b[128..].to_vec();
    b.extend(second);
    b[96..104].copy_from_slice(&48u64.to_le_bytes());
    assert!(inspect(&b).is_err());
    let mut b = image(&[7; 8]);
    b[136..140].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(inspect(&b).unwrap().build_id, None);
}
