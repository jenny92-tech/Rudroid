//! INPUT:  Bionic LP64 FILE ABI, shared extracted asset paths and host libc stdio
//! OUTPUT:  shared __sF/standard globals and adapted stdio function addresses
//! POS:    one 152-byte facade references one native stream and its sole buffer

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;
use std::sync::Once;

// Android android-15.0.0_r1 libc/stdio/local.h LP64 binary contract.
// Buffer internals/extensions are opaque here: guest must use stdio functions,
// not Bionic's private buffer callbacks or mutate the ownership cookie.
#[repr(C)]
pub(crate) struct File {
    p: *mut u8,
    r: c_int,
    w: c_int,
    flags: c_int,
    fd: c_int,
    bf: Buffer,
    lbfsize: c_int,
    cookie: *mut libc::FILE,
    close: *mut c_void,
    read: *mut c_void,
    seek: *mut c_void,
    write: *mut c_void,
    ext: Buffer,
    up: *mut u8,
    ur: c_int,
    ubuf: [u8; 3],
    nbuf: [u8; 1],
    lb: Buffer,
    blksize: c_int,
    unused: i64,
}
#[repr(C)]
struct Buffer {
    base: *mut u8,
    size: usize,
}
// SAFETY: every `File` field is an integer, byte array or raw pointer; all-zero is valid.
const EMPTY: File = unsafe { std::mem::zeroed() };
const _: () = assert!(std::mem::size_of::<File>() == 152);
const _: () = assert!(std::mem::align_of::<File>() == 8);
const _: () = assert!(std::mem::offset_of!(File, cookie) == 48);

static INIT: Once = Once::new();
// Guest-visible ABI globals: `__sF` and the `stdin`/`stdout`/`stderr`
// pointer variables are exported by address and the guest may read or
// reassign them, so they must be plain mutable statics. They are only ever
// touched through `addr_of!`/`addr_of_mut!` raw pointers, never references.
static mut STANDARD: [File; 3] = [EMPTY, EMPTY, EMPTY];
static mut INPUT: *mut File = ptr::null_mut();
static mut OUTPUT: *mut File = ptr::null_mut();
static mut ERROR: *mut File = ptr::null_mut();

unsafe extern "C" {
    fn rudroid_bionic_host_standard(index: u32) -> *mut libc::FILE;
}

fn standards() -> *mut File {
    // SAFETY: runs once before any reader; writes go through raw pointers into the statics.
    INIT.call_once(|| unsafe {
        let base = ptr::addr_of_mut!(STANDARD).cast::<File>();
        for index in 0..3 {
            let file = base.add(index);
            ptr::addr_of_mut!((*file).cookie).write(rudroid_bionic_host_standard(index as u32));
            ptr::addr_of_mut!((*file).fd).write(index as c_int);
            ptr::addr_of_mut!((*file).flags).write([4, 8, 10][index]);
        }
        ptr::addr_of_mut!(INPUT).write(base);
        ptr::addr_of_mut!(OUTPUT).write(base.add(1));
        ptr::addr_of_mut!(ERROR).write(base.add(2));
    });
    ptr::addr_of_mut!(STANDARD).cast()
}

/// # Safety
/// A non-null guest must be a live facade from this provider. The caller must
/// exclude fclose/free/cookie mutation for the whole subsequent native call.
/// As with C stdio, concurrent reads/writes use native FILE locking; closing
/// a stream while another thread uses or locks it is not supported.
#[no_mangle]
pub unsafe extern "C" fn rudroid_bionic_stdio_native(guest: *mut File) -> *mut libc::FILE {
    let host = if guest.is_null() {
        ptr::null_mut()
    } else {
        ptr::addr_of!((*guest).cookie).read()
    };
    if host.is_null() {
        *libc::__errno_location() = libc::EBADF;
    }
    host
}

#[no_mangle]
pub extern "C" fn rudroid_bionic_stdio_standard(index: u32) -> *mut File {
    standards();
    // SAFETY: `standards()` initialized the pointer statics; read by value via raw pointers.
    unsafe {
        match index {
            0 => ptr::addr_of!(INPUT).read(),
            1 => ptr::addr_of!(OUTPUT).read(),
            2 => ptr::addr_of!(ERROR).read(),
            _ => ptr::null_mut(),
        }
    }
}

unsafe fn wrap(host: *mut libc::FILE, mode: *const c_char) -> *mut File {
    if host.is_null() {
        return ptr::null_mut();
    }
    // Only this plain C-layout object belongs to the C heap; no Rust Box/Vec
    // or borrowed Rust payload ever enters the guest free/fclose path.
    let facade = libc::calloc(1, std::mem::size_of::<File>()).cast::<File>();
    if facade.is_null() {
        libc::fclose(host);
        *libc::__errno_location() = libc::ENOMEM;
        return ptr::null_mut();
    }
    let flags = if mode.is_null() {
        16
    } else {
        let mode = CStr::from_ptr(mode).to_bytes();
        if mode.contains(&b'+') {
            16
        } else if mode.first() == Some(&b'r') {
            4
        } else {
            8
        }
    };
    ptr::addr_of_mut!((*facade).cookie).write(host);
    ptr::addr_of_mut!((*facade).fd).write(libc::fileno(host));
    ptr::addr_of_mut!((*facade).flags).write(flags);
    facade
}
unsafe extern "C" fn open(path: *const c_char, mode: *const c_char) -> *mut File {
    let mapped = super::posix::rudroid_bionic_asset_path(path);
    // SAFETY: mapped is native-owned or null; the borrowed path/mode and
    // mapped storage remain live through fopen, then only mapped is freed.
    let host = libc::fopen(if mapped.is_null() { path } else { mapped }, mode);
    let saved_errno = *libc::__errno_location();
    libc::free(mapped.cast());
    *libc::__errno_location() = saved_errno;
    wrap(host, mode)
}
unsafe extern "C" fn fdopen(fd: c_int, mode: *const c_char) -> *mut File {
    wrap(libc::fdopen(fd, mode), mode)
}
unsafe extern "C" fn tmpfile() -> *mut File {
    wrap(libc::tmpfile(), ptr::null())
}

unsafe extern "C" fn close(file: *mut File) -> c_int {
    let host = rudroid_bionic_stdio_native(file);
    if host.is_null() {
        return -1;
    }
    let base = standards();
    let standard = file == base || file == base.add(1) || file == base.add(2);
    let result = libc::fclose(host);
    let saved_errno = *libc::__errno_location();
    // fclose consumes the stream even on a flush error. Static standard
    // facades/globals retain their address; never free the __sF array.
    if standard {
        ptr::addr_of_mut!((*file).cookie).write(ptr::null_mut());
        ptr::addr_of_mut!((*file).flags).write(0);
        ptr::addr_of_mut!((*file).fd).write(-1);
    } else {
        libc::free(file.cast());
    }
    // Older supported glibc versions do not promise free preserves errno.
    // Keep a native flush/close error visible after facade deallocation.
    *libc::__errno_location() = saved_errno;
    result
}

// Raw C pointers are borrowed only for a native call; no Rust shared/mutable
// references are constructed over guest-writable memory. Hot calls perform
// one cookie read, no Rust allocation, lookup, buffer copy or extra lock.
macro_rules! bridge {
    ($wrapper:ident, $host:ident, ($($arg:ident: $ty:ty),*) -> $ret:ty, $failure:expr) => {
        bridge!($wrapper, $host, (); ($($arg: $ty),*) -> $ret, $failure);
    };
    ($wrapper:ident, $host:ident, ($($before:ident: $before_ty:ty),*); ($($after:ident: $after_ty:ty),*) -> $ret:ty, $failure:expr) => {
        unsafe extern "C" fn $wrapper($($before: $before_ty,)* file: *mut File, $($after: $after_ty),*) -> $ret {
            unsafe extern "C" {
                fn $host($($before: $before_ty,)* file: *mut libc::FILE, $($after: $after_ty),*) -> $ret;
            }
            // SAFETY: the caller provides a live facade and native arguments;
            // the resolved FILE is borrowed only for this call, in its original ABI order.
            let host = rudroid_bionic_stdio_native(file);
            if host.is_null() { $failure } else { $host($($before,)* host, $($after),*) }
        }
    };
}
bridge!(getchar, fgetc, () -> c_int, -1);
bridge!(putc, fputc, (value: c_int); () -> c_int, -1);
bridge!(ungetc, ungetc, (value: c_int); () -> c_int, -1);
bridge!(fputwc, rudroid_bionic_fputwc, (value: u32); () -> u32, u32::MAX);
bridge!(ungetwc, rudroid_bionic_ungetwc, (value: u32); () -> u32, u32::MAX);
bridge!(getwc, rudroid_bionic_fgetwc, () -> u32, u32::MAX);
bridge!(orientation, rudroid_bionic_fwide, (mode: c_int) -> c_int, 0);
bridge!(eof, feof, () -> c_int, 0);
bridge!(error, ferror, () -> c_int, 0);
bridge!(clear, clearerr, () -> (), ());
bridge!(fileno, fileno, () -> c_int, -1);
bridge!(seek, fseeko, (offset: i64, whence: c_int) -> c_int, -1);
bridge!(tell, ftello, () -> i64, -1);
bridge!(rewind, rewind, () -> (), ());
bridge!(lock, flockfile, () -> (), ());
bridge!(trylock, ftrylockfile, () -> c_int, -1);
bridge!(unlock, funlockfile, () -> (), ());
bridge!(read, fread, (buffer: *mut c_void, size: usize, count: usize); () -> usize, 0);
bridge!(write, fwrite, (buffer: *const c_void, size: usize, count: usize); () -> usize, 0);
bridge!(gets, fgets, (buffer: *mut c_char, size: c_int); () -> *mut c_char, ptr::null_mut());
bridge!(fputs, fputs, (text: *const c_char); () -> c_int, -1);
unsafe extern "C" fn flush(file: *mut File) -> c_int {
    // A null FILE means all process streams, including host-owned streams.
    if file.is_null() {
        return libc::fflush(ptr::null_mut());
    }
    let host = rudroid_bionic_stdio_native(file);
    if host.is_null() {
        -1
    } else {
        libc::fflush(host)
    }
}
bridge!(setvbuf, setvbuf, (buffer: *mut c_char, mode: c_int, size: usize) -> c_int, -1);
unsafe extern "C" fn setbuf(file: *mut File, buffer: *mut c_char) {
    // Bionic BUFSIZ is 1024; native setbuf assumes glibc's 8192 and would
    // overrun a caller's Android-sized buffer. Preserve the guest contract.
    let (mode, size) = if buffer.is_null() { (2, 0) } else { (0, 1024) };
    setvbuf(file, buffer, mode, size);
}
unsafe extern "C" fn getpos(file: *mut File, position: *mut i64) -> c_int {
    let offset = tell(file);
    if offset == -1 {
        -1
    } else {
        position.write(offset);
        0
    }
}
unsafe extern "C" fn setpos(file: *mut File, position: *const i64) -> c_int {
    // Android fpos_t is off_t, not the larger glibc fpos_t state object.
    seek(file, position.read(), libc::SEEK_SET)
}
unsafe extern "C" fn stdout_putc(value: c_int) -> c_int {
    putc(value, rudroid_bionic_stdio_standard(1))
}
unsafe extern "C" fn stdin_getc() -> c_int {
    getchar(rudroid_bionic_stdio_standard(0))
}
unsafe extern "C" fn puts(text: *const c_char) -> c_int {
    let host = rudroid_bionic_stdio_native(rudroid_bionic_stdio_standard(1));
    if host.is_null() {
        return -1;
    }
    unsafe extern "C" {
        fn flockfile(file: *mut libc::FILE);
        fn funlockfile(file: *mut libc::FILE);
    }
    flockfile(host);
    let result = if libc::fputs(text, host) < 0 || libc::fputc(b'\n' as c_int, host) < 0 {
        -1
    } else {
        0
    };
    funlockfile(host);
    result
}

// Caller has already checked the library/version; no generic native lookup.
// Formatting entry points without a FILE are the separate stdio_variadic.c table.
symbol_provider! {
    unsafe;
    b"__sF" => standards() as usize,
    b"stdin" => {
        standards();
        ptr::addr_of_mut!(INPUT) as usize
    },
    b"stdout" => {
        standards();
        ptr::addr_of_mut!(OUTPUT) as usize
    },
    b"stderr" => {
        standards();
        ptr::addr_of_mut!(ERROR) as usize
    },
    b"fopen" | b"fopen64" => open as *const () as usize,
    b"fdopen" => fdopen as *const () as usize,
    b"tmpfile" | b"tmpfile64" => tmpfile as *const () as usize,
    b"fclose" => close as *const () as usize,
    b"fgetc" | b"getc" => getchar as *const () as usize,
    b"fputc" | b"putc" => putc as *const () as usize,
    b"fgetwc" | b"getwc" => getwc as *const () as usize,
    b"fputwc" | b"putwc" => fputwc as *const () as usize,
    b"ungetwc" => ungetwc as *const () as usize,
    b"fwide" => orientation as *const () as usize,
    b"ungetc" => ungetc as *const () as usize,
    b"feof" => eof as *const () as usize,
    b"ferror" => error as *const () as usize,
    b"clearerr" => clear as *const () as usize,
    b"fileno" => fileno as *const () as usize,
    b"fseek" | b"fseeko" | b"fseeko64" => seek as *const () as usize,
    b"ftell" | b"ftello" | b"ftello64" => tell as *const () as usize,
    b"rewind" => rewind as *const () as usize,
    b"fgetpos" => getpos as *const () as usize,
    b"fsetpos" => setpos as *const () as usize,
    b"flockfile" => lock as *const () as usize,
    b"ftrylockfile" => trylock as *const () as usize,
    b"funlockfile" => unlock as *const () as usize,
    b"fread" => read as *const () as usize,
    b"fwrite" => write as *const () as usize,
    b"fgets" => gets as *const () as usize,
    b"fputs" => fputs as *const () as usize,
    b"puts" => puts as *const () as usize,
    b"fflush" => flush as *const () as usize,
    b"setbuf" => setbuf as *const () as usize,
    b"setvbuf" => setvbuf as *const () as usize,
    b"putchar" => stdout_putc as *const () as usize,
    b"getchar" => stdin_getc as *const () as usize,
}
