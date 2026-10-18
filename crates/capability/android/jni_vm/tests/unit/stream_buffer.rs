//! INPUT:  private items of `src/stream_buffer.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/stream_buffer.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::runtime_policy::{MemoryMode, MemoryPolicy};
use std::io::Write;

struct TestFile {
    path: std::path::PathBuf,
}
impl TestFile {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rudroid-stream-policy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(
            &path,
            (0..65536).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        Self { path }
    }
    fn open(&self, budget: &Arc<BufferBudget>) -> FileStream {
        FileStream::new(std::fs::File::open(&self.path).unwrap(), &self.path, budget)
    }
    fn budget(&self, bytes: usize, limit: usize) -> Arc<BufferBudget> {
        Arc::new(BufferBudget {
            per_stream: bytes,
            limit,
            used: AtomicUsize::new(0),
            asset_root: Some(self.path.canonicalize().unwrap()),
        })
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn memory_modes_reduce_small_asset_file_reads() {
    let file = TestFile::new();
    for mode in [
        MemoryMode::LowMemory,
        MemoryMode::Balanced,
        MemoryMode::Performance,
    ] {
        let policy = MemoryPolicy::for_mode(mode);
        let budget = file.budget(
            policy.stream_buffer_bytes,
            policy.stream_buffer_budget_mib * 1024 * 1024,
        );
        let mut stream = file.open(&budget);
        let started = std::time::Instant::now();
        for offset in (0..65536).step_by(8) {
            let mut bytes = [0u8; 8];
            stream.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, std::array::from_fn(|i| ((offset + i) % 251) as u8));
        }
        let expected = match mode {
            MemoryMode::LowMemory => 8192,
            MemoryMode::Balanced => 8,
            MemoryMode::Performance => 1,
        };
        assert_eq!(stream.file_reads, expected);
        assert_eq!(stream.stream_position().unwrap(), 65536);
        eprintln!("stream_policy mode={} requests=8192 file_reads={} elapsed_us={} live_buffer_bytes={}", mode.as_str(), stream.file_reads, started.elapsed().as_micros(), budget.used.load(Ordering::Relaxed));
        assert_eq!(stream.read(&mut [0; 1]).unwrap(), 0);
        drop(stream);
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn buffer_budget_saturation_release_and_logical_position() {
    let file = TestFile::new();
    let budget = file.budget(8192, 8192);
    let mut first = file.open(&budget);
    first.read_exact(&mut [0u8; 3]).unwrap();
    let mut second = file.open(&budget);
    second.read_exact(&mut [0u8; 1]).unwrap();
    assert!(second.buffer.is_empty(), "budget miss keeps direct reads");
    assert_eq!(budget.used.load(Ordering::Relaxed), 8192);
    assert_eq!(
        first.stream_position().unwrap(),
        3,
        "available sees logical position, not read-ahead"
    );
    let mut rest = Vec::new();
    first.read_to_end(&mut rest).unwrap();
    assert_eq!(rest.len(), 65533);
    assert_eq!(rest[0], 3);
    drop(first);
    let mut third = file.open(&budget);
    third.read_exact(&mut [0u8; 1]).unwrap();
    assert_eq!(third.buffer.len(), 8192);
    drop((second, third));
    assert_eq!(budget.used.load(Ordering::Relaxed), 0);
}

#[test]
fn ordinary_mutable_files_keep_direct_read_visibility() {
    let file = TestFile::new();
    let budget = Arc::new(BufferBudget {
        per_stream: 8192,
        limit: 8192,
        used: AtomicUsize::new(0),
        asset_root: None,
    });
    let mut stream = file.open(&budget);
    stream.read_exact(&mut [0u8; 1]).unwrap();
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&file.path)
        .unwrap();
    writer.seek(std::io::SeekFrom::Start(1)).unwrap();
    writer.write_all(&[99]).unwrap();
    let mut next = [0; 1];
    stream.read_exact(&mut next).unwrap();
    assert_eq!(next, [99]);
    assert_eq!(budget.used.load(Ordering::Relaxed), 0);
}

#[test]
fn large_asset_reads_do_not_allocate_optional_buffers() {
    let file = TestFile::new();
    let budget = file.budget(8192, 8192);
    let mut stream = file.open(&budget);
    let mut data = vec![0; 65536];
    stream.read_exact(&mut data).unwrap();
    assert!(stream.buffer.is_empty());
    assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    assert_eq!(data[65535], (65535 % 251) as u8);
}
