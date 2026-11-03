//! INPUT:  synthetic AArch64 ELF, real noexec directory and Linux smaps
//! OUTPUT:  native residency, retained-symbol/code and teardown acceptance probe
//! POS:    verifies one-time source-page release without invalidating ELF borrows

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

use android_elf_load::{fixture, ExecutableModule};
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::path::Path;

const BYTES: usize = 64 * 1024 * 1024;

fn fixture_image() -> Vec<u8> {
    let mut image = fixture::build_minimal_aarch64_so();
    image.resize(BYTES, 0x5a);
    // One RX PT_LOAD, including padding, so the noexec copy actually reads
    // all 64 MiB. Relocation slots are separate from code and ELF tables.
    image[68..72].copy_from_slice(&5u32.to_le_bytes());
    image[96..104].copy_from_slice(&(BYTES as u64).to_le_bytes());
    image[104..112].copy_from_slice(&(BYTES as u64).to_le_bytes());
    image[512..516].copy_from_slice(&0x5280_0540u32.to_le_bytes()); // mov w0, #42
    image[516..520].copy_from_slice(&0xd65f_03c0u32.to_le_bytes()); // ret
    image[528..536].copy_from_slice(&1024u64.to_le_bytes());
    image[552..560].copy_from_slice(&1032u64.to_le_bytes());
    // A separate section-symbol fallback forces a post-advice read outside
    // the dynamic symbol tables, while preserving the same backing mapping.
    image[40..48].copy_from_slice(&2048u64.to_le_bytes());
    image[58..60].copy_from_slice(&64u16.to_le_bytes());
    image[60..62].copy_from_slice(&3u16.to_le_bytes());
    image[2048..2240].fill(0);
    let sym = 2048 + 64;
    image[sym + 4..sym + 8].copy_from_slice(&2u32.to_le_bytes());
    image[sym + 24..sym + 32].copy_from_slice(&3072u64.to_le_bytes());
    image[sym + 32..sym + 40].copy_from_slice(&24u64.to_le_bytes());
    image[sym + 40..sym + 44].copy_from_slice(&2u32.to_le_bytes());
    let strings = 2048 + 128;
    image[strings + 4..strings + 8].copy_from_slice(&3u32.to_le_bytes());
    image[strings + 24..strings + 32].copy_from_slice(&3104u64.to_le_bytes());
    image[strings + 32..strings + 40].copy_from_slice(&8u64.to_le_bytes());
    image[3072..3096].fill(0);
    image[3072..3076].copy_from_slice(&1u32.to_le_bytes());
    image[3076] = 0x02; // local function, only visible to symbol_all
    image[3078..3080].copy_from_slice(&1u16.to_le_bytes());
    image[3080..3088].copy_from_slice(&512u64.to_le_bytes());
    image[3104..3112].copy_from_slice(b"\0hidden\0");
    image
}

fn mapping(path: &Path, permissions: &str) -> Option<(usize, usize, usize)> {
    let maps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut found = None;
    for line in maps.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first().is_some_and(|first| first.contains('-')) {
            if fields.get(1) == Some(&permissions) && line.ends_with(path.to_str().unwrap()) {
                let (start, end) = fields[0].split_once('-').unwrap();
                found = Some((
                    usize::from_str_radix(start, 16).unwrap(),
                    usize::from_str_radix(end, 16).unwrap(),
                    0,
                ));
            } else if found.is_some() {
                break;
            }
        } else if fields.first() == Some(&"Rss:") {
            if let Some(ref mut row) = found {
                row.2 = fields[1].parse().unwrap();
            }
        }
    }
    found
}

#[test]
#[ignore = "run alone on native AArch64 with RUDROID_NOEXEC_FIXTURE_DIR on a real noexec mount"]
fn noexec_copy_releases_source_residency_without_unmapping_metadata() {
    let directory = std::env::var_os("RUDROID_NOEXEC_FIXTURE_DIR").expect("real noexec directory");
    let path = Path::new(&directory).join(format!(
        "rudroid-noexec-residency-{}.so",
        std::process::id()
    ));
    let bytes = fixture_image();
    std::fs::write(&path, &bytes).unwrap();
    drop(bytes);
    let file = std::fs::File::open(&path).unwrap();
    // SAFETY: a live fd and initialized output, checked before inspection.
    let noexec = unsafe {
        let mut filesystem: libc::statvfs = std::mem::zeroed();
        assert_eq!(libc::fstatvfs(file.as_raw_fd(), &mut filesystem), 0);
        filesystem.f_flag & libc::ST_NOEXEC != 0
    };
    assert!(noexec, "this probe must exercise the copied noexec path");
    drop(file);
    // SAFETY: fixture contains only freestanding trusted code; the backing
    // file remains unchanged and no guest threads or external callbacks exist.
    let module = unsafe { ExecutableModule::map(&path, &HashMap::new()) }.unwrap();
    let before = mapping(&path, "r--s").expect("retained read-only metadata mapping");
    assert_eq!(before.1 - before.0, BYTES);
    assert!(
        before.2 < 1024,
        "source RSS should be below 1 MiB, not a second 64 MiB copy: {before:?}"
    );
    for _ in 0..1000 {
        let address = module.symbol(fixture::ANSWER_SYMBOL).unwrap();
        assert_eq!(module.symbol_all("hidden").unwrap(), Some(address));
        assert!(module.symbol("absent").is_none());
        // SAFETY: the fixture export is exactly unsigned answer(void).
        let answer: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(address) };
        assert_eq!(unsafe { answer() }, 42);
    }
    // SAFETY: these aligned slots and sentinel lie inside the live RX image.
    unsafe {
        assert_eq!(
            ((module.base() + 1024) as *const usize).read(),
            module.base() + 0x100
        );
        assert_eq!(((module.base() + 1032) as *const usize).read(), 0);
        assert_eq!(((module.base() + BYTES - 1) as *const u8).read(), 0x5a);
        // Program-header pointers remain readable at their original address.
        let header = ((before.0 + 64) as *const libc::Elf64_Phdr).read();
        assert_eq!(header.p_type, 1);
        assert_eq!(header.p_filesz, BYTES as u64);
    }
    let after = mapping(&path, "r--s").unwrap();
    assert_eq!((before.0, before.1), (after.0, after.1));
    assert!(
        after.2 < 1024,
        "metadata queries must not reload the entire file: {after:?}"
    );
    println!(
        "NOEXEC_RESIDENCY bytes={BYTES} initial_source_rss_kib={} queried_source_rss_kib={}",
        before.2, after.2
    );
    drop(module);
    assert!(
        mapping(&path, "r--s").is_none(),
        "last owner unmaps metadata"
    );
    // Delete only this process's synthetic fixture, after all mappings die.
    std::fs::remove_file(path).unwrap();
}
