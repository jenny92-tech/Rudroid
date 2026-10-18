//! INPUT:  compiler-generated training cfg and LLVM's per-image profiling runtime
//! OUTPUT:  training identity, checked profile write and conditional DSO export
//! POS:    collects host-only PGO counters without guest finalizers or shared DSO statics
pub const TRAINING: bool = cfg!(rudroid_pgo_training);
pub const FLUSH_SYMBOL: &str = "rudroid_pgo_training_flush";

/// Write this image's counters after its owned workers have stopped. Each
/// instrumented image owns a hidden LLVM runtime; no guest state is traversed.
pub fn flush() -> Result<(), i32> {
    #[cfg(rudroid_pgo_training)]
    {
        extern "C" { fn __llvm_profile_write_file() -> i32; }
        // SAFETY: the fixed entry links LLVM's profiling runtime into this
        // image. Its documented no-argument C interface owns all buffers and
        // file handles internally; neither Rust nor guest pointers cross it.
        let result = unsafe { __llvm_profile_write_file() };
        if result != 0 { return Err(result); }
    }
    Ok(())
}

// Select the macro in common, not the consuming crate: capability manifests
// do not each need an independent feature that could desynchronize the kit.
#[cfg(rudroid_pgo_training)]
#[macro_export]
macro_rules! export_pgo_training {
    () => {
        #[no_mangle]
        extern "C" fn rudroid_pgo_training_flush() -> i32 {
            $crate::pgo::flush().err().unwrap_or(0)
        }
    };
}
#[cfg(not(rudroid_pgo_training))]
#[macro_export]
macro_rules! export_pgo_training { () => {}; }
