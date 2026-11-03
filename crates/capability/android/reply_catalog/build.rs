//! Build the Reply Catalog binary blob from the in-repo source table.
//!
//! Rebuild-on-change: cargo reruns this script whenever the source table
//! changes, so the embedded blob always matches the code; no format compat
//! matrix is needed.

use rudroid_reply_catalog_format::{encode, source::SOURCE_TABLE, Entry};

fn main() {
    // The table is in the `format` build-dependency; cargo reruns this script
    // whenever that crate changes.
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = std::path::PathBuf::from(
        std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"),
    );

    let entries: Vec<Entry<'_>> = SOURCE_TABLE
        .iter()
        .map(|e| Entry {
            class: e.class,
            method: e.method,
            sig: e.sig,
            value: e.value.clone(),
        })
        .collect();
    let blob = encode(&entries);
    let blob_path = out_dir.join("reply_catalog.bin");
    std::fs::write(&blob_path, &blob).expect("write reply_catalog.bin");

    println!(
        "cargo:rustc-env=RUDROID_REPLY_CATALOG_BLOB_PATH={}",
        blob_path.display()
    );
}
