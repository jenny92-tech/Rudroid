//! Ticket 90 closeout: query-cost profile of the shipped Catalog index
//! (sorted Vec + `binary_search_by`). Measures ns/query for the REAL
//! embedded table (SOURCE_TABLE) and a large synthetic one, hit and miss
//! paths, so the ledger carries a measured baseline of the CURRENT
//! scheme.
//!
//! Honesty notes (ticket 90 gate): this is NOT a comparison against the
//! historical hash-map scheme — that implementation no longer exists in
//! the tree and rebuilding it for comparison would manufacture a baseline
//! the ledger does not need; the ticket's old claim concerned allocations,
//! which this probe does not re-litigate. Numbers are Debug-profile
//! container measurements unless the run says Release; both are recorded
//! when both are run. Absolute ns values are environment-bound; only
//! orders of magnitude and hit-vs-miss shape transfer.

use rudroid_reply_catalog_format::catalog::Catalog;
use rudroid_reply_catalog_format::{encode, Entry, ReplyValue};
use std::time::Instant;

fn timed_queries(catalog: &Catalog, class: &str, method: &str, sig: &str, rounds: u32) -> f64 {
    // Warm once (page in, branch caches).
    let _ = catalog.query(class, method, sig);
    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..rounds {
        // The query returns a reference; touch it so the lookup cannot be
        // dead-code eliminated.
        if catalog.query(class, method, sig).is_some() {
            sink += 1;
        }
    }
    let _ = std::hint::black_box(sink);
    start.elapsed().as_nanos() as f64 / rounds as f64
}

#[test]
fn query_cost_profile_hit_and_miss_small_and_large() {
    // The real embedded table.
    let small = Catalog::from_source();
    let small_len = small.len();
    // A large synthetic table: 50k entries.
    let big_entries: Vec<Entry<'_>> = (0..50_000)
        .map(|i| Entry {
            class: Box::leak(format!("com/big/Class{i:05}").into_boxed_str()),
            method: Box::leak(format!("method{i:05}").into_boxed_str()),
            sig: Box::leak(format!("(I){i:05}").into_boxed_str()),
            value: ReplyValue::Int(i as i64),
        })
        .collect();
    let big = Catalog::from_blob(&encode(&big_entries)).expect("big blob decodes");

    let mut report = String::from("T90 CATALOG COST (ns/query, warm in-process):");
    for (label, catalog, probe) in [
        (
            format!("small(SOURCE_TABLE n={small_len})"),
            &small,
            ("org/fmod", "checkInit", "()Z"),
        ),
        (
            format!("big(n={})", big.len()),
            &big,
            ("com/big/Class25000", "method25000", "(I)25000"),
        ),
    ] {
        let hit = timed_queries(catalog, probe.0, probe.1, probe.2, 200_000);
        let miss = timed_queries(catalog, "com/absent", "no_method", "()V", 200_000);
        report.push_str(&format!(
            "\n  {label}: hit {hit:.0} ns, miss {miss:.0} ns"
        ));
    }
    eprintln!("{report}");
    // The invariant the scheme must keep: lookups are read-only and total
    // (no panic on miss) — the cost numbers above are the ledger payload.
}

/// Building the big table also pins the construct cost (sort included) —
/// the startup-side number the ledger records alongside query cost.
#[test]
fn construct_cost_profile() {
    let big_entries: Vec<Entry<'_>> = (0..50_000)
        .map(|i| Entry {
            class: Box::leak(format!("com/big/Class{i:05}").into_boxed_str()),
            method: Box::leak(format!("method{i:05}").into_boxed_str()),
            sig: Box::leak(format!("(I){i:05}").into_boxed_str()),
            value: ReplyValue::Int(i as i64),
        })
        .collect();
    let blob = encode(&big_entries);
    let start = Instant::now();
    let catalog = Catalog::from_blob(&blob).expect("decodes");
    let elapsed_ms = start.elapsed().as_millis();
    eprintln!(
        "T90 CATALOG CONSTRUCT: {} entries in {elapsed_ms} ms (decode + last-wins dedupe + sort)",
        catalog.len()
    );
    assert_eq!(catalog.len(), 50_000);
}
