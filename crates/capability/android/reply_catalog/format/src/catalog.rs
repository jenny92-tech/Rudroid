//! Queryable catalog model shared by every consumer of fixed replies.
//!
//! This is plain-library code: no descriptor export, no cdylib, no embedded
//! blob. Capability crates (`android_jni_vm`, `android_base`) depend on these
//! types instead of statically linking the `reply_catalog` capability, whose
//! same-build blob stays behind its own dynamic C API.
#![forbid(unsafe_code)]

use std::collections::HashMap;

use super::{decode, encode, DecodeError, Entry, ReplyValue};
use super::source::SOURCE_TABLE;

/// Queryable catalog: `(class, method, sig)` → fixed reply.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    entries: Vec<((String, String, String), ReplyValue)>,
}

impl Catalog {
    /// Decode a catalog blob.
    pub fn from_blob(bytes: &[u8]) -> Result<Catalog, DecodeError> {
        let decoded = decode(bytes)?;
        let mut entries = HashMap::with_capacity(decoded.len());
        for (class, method, sig, value) in decoded {
            entries.insert(
                (class.to_owned(), method.to_owned(), sig.to_owned()),
                value,
            );
        }
        // Preserve last-entry-wins decoding, then freeze the read-only index.
        let mut entries: Vec<_> = entries.into_iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        Ok(Catalog { entries })
    }

    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Catalog, CatalogLoadError> {
        let bytes = std::fs::read(path).map_err(CatalogLoadError::Io)?;
        Catalog::from_blob(&bytes).map_err(CatalogLoadError::Format)
    }

    /// Catalog built from the in-repo source table (same data the build script
    /// embeds for the cdylib C API).
    pub fn from_source() -> Catalog {
        let entries: Vec<Entry<'_>> = SOURCE_TABLE
            .iter()
            .map(|e| Entry {
                class: e.class,
                method: e.method,
                sig: e.sig,
                value: e.value.clone(),
            })
            .collect();
        Catalog::from_blob(&encode(&entries)).expect("source table is valid")
    }

    /// Exact-key lookup. `None` means "no configured reply" — callers apply
    /// their own type default (documented missing-key behavior).
    pub fn query(&self, class: &str, method: &str, sig: &str) -> Option<&ReplyValue> {
        let index = self.entries.binary_search_by(|((c, m, s), _)| {
            (c.as_str(), m.as_str(), s.as_str()).cmp(&(class, method, sig))
        }).ok()?;
        Some(&self.entries[index].1)
    }

    pub fn query_bool(&self, class: &str, method: &str, sig: &str, fallback: bool) -> bool {
        match self.query(class, method, sig) {
            Some(ReplyValue::Bool(b)) => *b,
            _ => fallback,
        }
    }

    pub fn query_int(&self, class: &str, method: &str, sig: &str, fallback: i64) -> i64 {
        match self.query(class, method, sig) {
            Some(ReplyValue::Int(i)) => *i,
            _ => fallback,
        }
    }

    pub fn query_str(&self, class: &str, method: &str, sig: &str, fallback: &str) -> String {
        match self.query(class, method, sig) {
            Some(ReplyValue::Str(s)) => s.clone(),
            _ => fallback.to_owned(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug)]
pub enum CatalogLoadError {
    Io(std::io::Error),
    Format(DecodeError),
}

impl std::fmt::Display for CatalogLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogLoadError::Io(e) => write!(f, "cannot read catalog blob: {e}"),
            CatalogLoadError::Format(e) => write!(f, "invalid catalog blob: {e}"),
        }
    }
}

impl std::error::Error for CatalogLoadError {}

#[cfg(test)]
#[path = "../tests/unit/catalog.rs"]
mod tests;
