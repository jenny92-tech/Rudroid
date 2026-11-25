//! INPUT:  Java StatFs construction and refresh requests
//! OUTPUT: legacy C++ compatibility capacity: 4096-byte blocks, 32 GiB total/free/available
//! POS:    Android query facade; backing filesystem and write failures are unchanged
#![forbid(unsafe_code)]
use rudroid_common::{
    jni_abi::declare_field,
    jni_env::{JObject, JniEnv, Native},
};
use std::{
    ffi::{c_void, CStr},
    sync::OnceLock,
};
const COMPAT_BLOCK_SIZE: i64 = 4096;
const COMPAT_CAPACITY_BYTES: i64 = 32 * 1024 * 1024 * 1024;
static SNAPSHOT: OnceLock<usize> = OnceLock::new();
fn error(env: JniEnv, class: &CStr, message: &CStr) {
    if env.exception_check() {
        return;
    }
    let kind = env.find_class(class);
    env.throw_new(kind, message);
    env.delete_local_ref(kind);
}
extern "C" fn restat(env: JniEnv, object: JObject, _path: JObject) {
    // Match the legacy C++ capacity view used by game startup checks. Path
    // arguments are intentionally ignored; no filesystem is queried/created.
    // Host FilesystemStats and actual file writes retain their real semantics.
    let blocks = COMPAT_CAPACITY_BYTES / COMPAT_BLOCK_SIZE;
    let values = [
        COMPAT_BLOCK_SIZE,
        blocks,
        blocks,
        blocks,
    ];
    let array = env.new_long_array(4);
    if array.is_null() || env.exception_check() {
        env.delete_local_ref(array);
        return;
    }
    env.set_long_array_region(array, 0, &values);
    if !env.exception_check() {
        // Publish a complete VM-owned view; refresh releases the previous one.
        env.set_object_field(object, *SNAPSHOT.get().unwrap() as *mut c_void, array);
    }
    env.delete_local_ref(array);
}
fn snapshot(env: JniEnv, object: JObject) -> Option<[i64; 4]> {
    let array = env.get_object_field(object, *SNAPSHOT.get().unwrap() as *mut c_void);
    if array.is_null() {
        error(
            env,
            c"java/lang/IllegalStateException",
            c"StatFs has no successful snapshot",
        );
        return None;
    }
    let mut values = [0; 4];
    env.get_long_array_region(array, 0, &mut values);
    env.delete_local_ref(array);
    (!env.exception_check()).then_some(values)
}
macro_rules! blocks {
    ($long:ident,$int:ident,$index:expr) => {
        extern "C" fn $long(env: JniEnv, object: JObject) -> i64 {
            snapshot(env, object).map_or(0, |s| s[$index])
        }
        extern "C" fn $int(env: JniEnv, object: JObject) -> i32 {
            $long(env, object) as i32
        }
    };
}
blocks!(block_size, size, 0);
blocks!(block_count, count, 1);
blocks!(free_blocks, free, 2);
blocks!(available_blocks, available, 3);
macro_rules! bytes {
    ($name:ident,$index:expr) => {
        extern "C" fn $name(env: JniEnv, object: JObject) -> i64 {
            snapshot(env, object).map_or(0, |s| s[0].wrapping_mul(s[$index]))
        }
    };
}
bytes!(total_bytes, 1);
bytes!(free_bytes, 2);
bytes!(available_bytes, 3);
pub(super) fn register(env: JniEnv) -> i32 {
    if declare_field(env, c"android/os/StatFs", c"_rudroidStat", c"[J", false) != 0 {
        return -1;
    }
    let class = env.find_class(c"android/os/StatFs");
    let field = env.get_field_id(class, c"_rudroidStat", c"[J");
    env.delete_local_ref(class);
    if field.is_null() || env.exception_check() {
        return -1;
    }
    SNAPSHOT.get_or_init(|| field as usize);
    env.register_class(
        c"android/os/StatFs",
        &[
            Native::new1(c"<init>", c"(Ljava/lang/String;)V", restat),
            Native::new1(c"restat", c"(Ljava/lang/String;)V", restat),
            Native::new0(c"getBlockSize", c"()I", size),
            Native::new0(c"getBlockSizeLong", c"()J", block_size),
            Native::new0(c"getBlockCount", c"()I", count),
            Native::new0(c"getBlockCountLong", c"()J", block_count),
            Native::new0(c"getFreeBlocks", c"()I", free),
            Native::new0(c"getFreeBlocksLong", c"()J", free_blocks),
            Native::new0(c"getAvailableBlocks", c"()I", available),
            Native::new0(c"getAvailableBlocksLong", c"()J", available_blocks),
            Native::new0(c"getTotalBytes", c"()J", total_bytes),
            Native::new0(c"getFreeBytes", c"()J", free_bytes),
            Native::new0(c"getAvailableBytes", c"()J", available_bytes),
        ],
    )
}
