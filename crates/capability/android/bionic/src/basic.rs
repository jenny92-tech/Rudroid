//! INPUT:  verified ARM64 C scalar/pointer APIs and the process native libc
//! OUTPUT:  native memory/string/FORTIFY addresses, errno and old-glibc adapters
//! POS:    one shared libc service; no per-call registry, buffer or Rust allocation

use std::ffi::{c_char, c_int, c_void};

// Both Android and GNU use four LP64 scalar/pointer arguments. Keep GNU's
// actual destination-bound check and fatal path instead of forwarding these
// imports to unchecked memcpy/memmove/memset. No Rust owner crosses this ABI.
extern "C" {
    #[link_name="__memcpy_chk"]
    fn memcpy_chk(dst:*mut c_void,src:*const c_void,n:usize,capacity:usize)->*mut c_void;
    #[link_name="__memmove_chk"]
    fn memmove_chk(dst:*mut c_void,src:*const c_void,n:usize,capacity:usize)->*mut c_void;
    #[link_name="__memset_chk"]
    fn memset_chk(dst:*mut c_void,value:c_int,n:usize,capacity:usize)->*mut c_void;
}

symbol_provider! {
    b"__memcpy_chk" => memcpy_chk as *const () as usize,
    b"__memmove_chk" => memmove_chk as *const () as usize,
    b"__memset_chk" => memset_chk as *const () as usize,
    b"memcpy" => libc::memcpy as *const () as usize,
    b"memmove" => libc::memmove as *const () as usize,
    b"memset" => libc::memset as *const () as usize,
    b"memcmp" => libc::memcmp as *const () as usize,
    b"memchr" => libc::memchr as *const () as usize,
    b"memrchr" => libc::memrchr as *const () as usize,
    b"strlen" => libc::strlen as *const () as usize,
    b"strnlen" => libc::strnlen as *const () as usize,
    b"strcmp" => libc::strcmp as *const () as usize,
    b"strncmp" => libc::strncmp as *const () as usize,
    b"strchr" => libc::strchr as *const () as usize,
    b"strrchr" => libc::strrchr as *const () as usize,
    b"strstr" => libc::strstr as *const () as usize,
    b"strcpy" => libc::strcpy as *const () as usize,
    b"strncpy" => libc::strncpy as *const () as usize,
    b"strcat" => libc::strcat as *const () as usize,
    b"strncat" => libc::strncat as *const () as usize,
    b"strdup" => libc::strdup as *const () as usize,
    b"strndup" => libc::strndup as *const () as usize,
    b"strlcpy" => strlcpy as *const () as usize,
    b"strlcat" => strlcat as *const () as usize,
    b"strtol" => libc::strtol as *const () as usize,
    b"strtoll" => libc::strtoll as *const () as usize,
    b"strtoul" => libc::strtoul as *const () as usize,
    b"strtoull" => libc::strtoull as *const () as usize,
    b"strtof" => libc::strtof as *const () as usize,
    b"strtod" => libc::strtod as *const () as usize,
    b"atoi" => libc::atoi as *const () as usize,
    b"atol" => libc::atol as *const () as usize,
    b"atoll" => libc::atoll as *const () as usize,
    b"qsort" => libc::qsort as *const () as usize,
    b"bsearch" => libc::bsearch as *const () as usize,
    b"__errno" => libc::__errno_location as *const () as usize,
    b"gettid" => gettid as *const () as usize,
    b"syscall" => libc::syscall as *const () as usize,
    b"getpid" => libc::getpid as *const () as usize,
    b"getppid" => libc::getppid as *const () as usize,
    b"getuid" => libc::getuid as *const () as usize,
    b"geteuid" => libc::geteuid as *const () as usize,
    b"getgid" => libc::getgid as *const () as usize,
    b"getegid" => libc::getegid as *const () as usize,
    b"exit" => libc::exit as *const () as usize,
    b"_exit" => libc::_exit as *const () as usize,
    b"getenv" => libc::getenv as *const () as usize,
    b"setenv" => libc::setenv as *const () as usize,
    b"unsetenv" => libc::unsetenv as *const () as usize,
    b"putenv" => libc::putenv as *const () as usize,
    // Do not forward struct/locale/multibyte/pthread/sysconf APIs just
    // because a same-named glibc function exists: their Android ABI differs.
}

unsafe extern "C" fn gettid() -> c_int {
    // Linux AArch64 syscall ABI matches Android. The glibc gettid wrapper
    // requires 2.30; this already-supported kernel call avoids that dependency.
    libc::syscall(libc::SYS_gettid) as c_int
}

unsafe extern "C" fn strlcpy(
    destination: *mut c_char,
    source: *const c_char,
    size: usize,
) -> usize {
    // Native strlcpy/strlcat require glibc 2.38. Retain C pointer ownership
    // and bounded writes without raising the handheld's libc requirement.
    let length = libc::strlen(source);
    if size != 0 {
        let copied = length.min(size - 1);
        std::ptr::copy_nonoverlapping(source, destination, copied);
        destination.add(copied).write(0);
    }
    length
}

unsafe extern "C" fn strlcat(
    destination: *mut c_char,
    source: *const c_char,
    size: usize,
) -> usize {
    let source_length = libc::strlen(source);
    let destination_length = if size == 0 {
        0
    } else {
        libc::strnlen(destination, size)
    };
    if destination_length < size {
        let copied = source_length.min(size - destination_length - 1);
        std::ptr::copy_nonoverlapping(source, destination.add(destination_length), copied);
        destination.add(destination_length + copied).write(0);
    }
    source_length.wrapping_add(destination_length)
}

#[cfg(test)]
#[path = "../tests/unit/basic.rs"]
mod tests;
