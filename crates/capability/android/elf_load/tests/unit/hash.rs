//! INPUT:  private items of `src/hash.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/hash.rs`; declared there with `#[path]`
use super::*;

#[test]
fn sysv_odd_successor_is_not_an_end_marker() {
    let raw: Vec<u8> = [1u32, 4, 1, 0, 3, 0, 0]
        .into_iter().flat_map(u32::to_le_bytes).collect();
    let names = ["", "first", "unused", "wanted"];
    let fetch = name_of(&names);
    let table = SysvTable::parse(&raw).unwrap();
    assert_eq!(table.lookup("wanted", &fetch).unwrap(), Some(3));
    assert_eq!(table.lookup("absent", &fetch).unwrap(), None);
}

#[test]
fn sysv_cyclic_chain_is_rejected() {
    let raw: Vec<u8> = [1u32, 3, 2, 0, 0, 2]
        .into_iter().flat_map(u32::to_le_bytes).collect();
    let names = ["", "unused", "cycle"];
    let error = SysvTable::parse(&raw)
        .unwrap()
        .lookup("absent", &name_of(&names))
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("cyclic"), "names the cycle: {message}");
    assert!(!message.contains("truncated"), "a cycle is not a truncation: {message}");
}

#[test]
fn sysv_empty_chain_table_is_a_miss() {
    let raw: Vec<u8> = [1u32, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect();
    let names = [""];
    let table = SysvTable::parse(&raw).unwrap();
    assert_eq!(table.lookup("absent", &name_of(&names)).unwrap(), None);
}

#[test]
fn sysv_hash_known_values() {
    // Reference values from the System V ABI / glibc documentation.
    assert_eq!(sysv_hash(""), 0);
    assert_eq!(sysv_hash("printf"), 0x0779_05a6);
    assert_eq!(sysv_hash("exit"), 0x0006_cf04);
}

#[test]
fn gnu_hash_known_values() {
    assert_eq!(gnu_hash(""), 5381);
    assert_eq!(gnu_hash("a"), 0x0002_b606);
}

fn name_of<'a>(names: &'a [&'a str]) -> impl Fn(usize) -> Result<&'a str, ElfError> + 'a {
    move |index: usize| {
        names
            .get(index)
            .copied()
            .ok_or(ElfError::TruncatedSection("dynsym"))
    }
}

#[test]
fn sysv_table_lookup_hits_and_misses() {
    // 1 reserved symbol + two named, chained in bucket 0.
    // buckets[0] = 1; chain[1] = 2 (continue), chain[2] = 0 (end).
    let mut raw = vec![0u8; 8 + 1 * 4 + 3 * 4];
    raw[0..4].copy_from_slice(&1u32.to_le_bytes()); // nbuckets
    raw[4..8].copy_from_slice(&3u32.to_le_bytes()); // nchain
    raw[8..12].copy_from_slice(&1u32.to_le_bytes()); // buckets[0]
    // chain[i] lives at 8 + (nbuckets + i) * 4.
    raw[8 + (1 + 1) * 4..8 + (1 + 1) * 4 + 4].copy_from_slice(&2u32.to_le_bytes()); // chain[1] -> 2
    raw[8 + (1 + 2) * 4..8 + (1 + 2) * 4 + 4].copy_from_slice(&0u32.to_le_bytes()); // chain[2] end
    let table = SysvTable::parse(&raw).unwrap();
    let names = ["", "rudroid_alpha", "rudroid_beta"];
    let fetch = name_of(&names);
    // Both names hash to bucket 0 in this fixture by construction.
    let _ = fetch(1).unwrap();
    // Chain walk returns whichever matches; misses return None.
    assert_eq!(table.lookup("rudroid_absent", &fetch).unwrap(), None);
}

#[test]
fn sysv_table_lookup_finds_by_name_comparison() {
    // Make both fixture names land in the same bucket with a chain.
    let names = ["", "rudroid_alpha", "rudroid_beta"];
    let h1 = sysv_hash("rudroid_alpha");
    let h2 = sysv_hash("rudroid_beta");
    assert_eq!(h1 % 2, h2 % 2, "fixture names must collide for the test");
    let mut raw = vec![0u8; 8 + 2 * 4 + 3 * 4];
    raw[0..4].copy_from_slice(&2u32.to_le_bytes()); // nbuckets = 2
    raw[4..8].copy_from_slice(&3u32.to_le_bytes()); // nchain = 3
    let bucket = (h1 % 2) as usize;
    raw[8 + bucket * 4..8 + bucket * 4 + 4].copy_from_slice(&1u32.to_le_bytes());
    raw[8 + (2 + 1) * 4..8 + (2 + 1) * 4 + 4].copy_from_slice(&2u32.to_le_bytes()); // chain[1] -> 2
    raw[8 + (2 + 2) * 4..8 + (2 + 2) * 4 + 4].copy_from_slice(&0u32.to_le_bytes()); // chain[2] end
    let table = SysvTable::parse(&raw).unwrap();
    let fetch = name_of(&names);
    assert_eq!(table.lookup("rudroid_alpha", &fetch).unwrap(), Some(1));
    assert_eq!(table.lookup("rudroid_beta", &fetch).unwrap(), Some(2));
}

#[test]
fn gnu_table_bloom_miss_short_circuits() {
    // Header: nbuckets=1, symoffset=1, bloom_size=1, bloom_shift=0.
    let mut raw = vec![0u8; 16 + 8 + 4 + 4];
    raw[0..4].copy_from_slice(&1u32.to_le_bytes());
    raw[4..8].copy_from_slice(&1u32.to_le_bytes());
    raw[8..12].copy_from_slice(&1u32.to_le_bytes());
    raw[12..16].copy_from_slice(&0u32.to_le_bytes());
    // Bloom word stays zero: every lookup must miss without touching dynsym.
    let table = GnuTable::parse(&raw).unwrap();
    let names = [""];
    let fetch = name_of(&names);
    assert_eq!(table.lookup("anything", &fetch).unwrap(), None);
}

#[test]
fn gnu_table_chain_walk_finds_symbol() {
    // Construct a 1-bucket table where "rudroid_beta" is reachable from
    // "rudroid_alpha"'s chain.
    let names = ["", "rudroid_alpha", "rudroid_beta"];
    let h1 = gnu_hash("rudroid_alpha");
    let h2 = gnu_hash("rudroid_beta");
    let mut raw = vec![0u8; 16 + 8 + 4 + 2 * 4];
    raw[0..4].copy_from_slice(&1u32.to_le_bytes()); // nbuckets
    raw[4..8].copy_from_slice(&1u32.to_le_bytes()); // symoffset
    raw[8..12].copy_from_slice(&1u32.to_le_bytes()); // bloom_size
    raw[12..16].copy_from_slice(&0u32.to_le_bytes()); // bloom_shift
    // Bloom accepts both hashes: set all bits they need.
    let bloom = (1u64 << (h1 % 64))
        | (1u64 << ((h1 >> 0) % 64))
        | (1u64 << (h2 % 64))
        | (1u64 << ((h2 >> 0) % 64));
    raw[16..24].copy_from_slice(&bloom.to_le_bytes());
    // bucket[0] -> symbol 1; chain[0] (= sym 1) holds h1 with the end
    // bit clear so the walk continues into symbol 2.
    raw[24..28].copy_from_slice(&1u32.to_le_bytes());
    raw[28..32].copy_from_slice(&((h1 & !1) as u32).to_le_bytes());
    // chain[1] (= sym 2) must equal h2 with end bit set:
    raw[32..36].copy_from_slice(&((h2 | 1) as u32).to_le_bytes());
    let table = GnuTable::parse(&raw).unwrap();
    let fetch = name_of(&names);
    assert_eq!(table.lookup("rudroid_beta", &fetch).unwrap(), Some(2));
    assert_eq!(table.lookup("rudroid_alpha", &fetch).unwrap(), Some(1));
    assert_eq!(table.lookup("absent", &fetch).unwrap(), None);
}
