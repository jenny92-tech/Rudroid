//! INPUT:  ELF file metadata and common's borrowed C ImportVisitor callback
//! OUTPUT:  rudroid_elf_visit_imports enumerates all relocation-used import tuples
//! POS:    cold diagnostic API reuses the ELF parser without executing guest code

use super::{elf::ElfView, LoadError};
use rudroid_common::symbols::elf_load::ImportVisitor;
use std::collections::BTreeSet;
use std::ffi::{c_char, c_void, CStr, CString};

/// # Safety
/// path must be a live NUL-terminated UTF-8 filename. visitor/user remain
/// valid for synchronous calls and must not unwind. String arguments are
/// borrowed only during each callback. No capability code is statically
/// linked by the caller; the ELF library must remain loaded during this call.
/// On metadata failure, earlier visits may have happened; return -1 is never
/// a completed inventory. No executable mapping/relocation/constructor runs.
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_visit_imports(
    path: *const c_char,
    visitor: Option<ImportVisitor>,
    user: *mut c_void,
) -> i32 {
    super::clear_last_error();
    if path.is_null() || visitor.is_none() {
        super::set_last_error(&format_args!("import inventory: null path or visitor"));
        return -1;
    }
    let Ok(path) = CStr::from_ptr(path).to_str() else {
        super::set_last_error(&format_args!("import inventory: path is not UTF-8"));
        return -1;
    };
    let result = (|| -> Result<(), LoadError> {
        let bytes = std::fs::read(path).map_err(LoadError::Io)?;
        let view = ElfView::parse(&bytes)?;
        let symbols = view
            .rela_iter()?
            .map(|entry| entry.2)
            .filter(|index| *index != 0)
            .collect::<BTreeSet<_>>();
        for index in symbols {
            let (name, _, section) = view.dynsym(index)?;
            if section != 0 {
                continue;
            }
            let version = view.symbol_version(index)?;
            // ELF strings were validated to end before their table boundary.
            let name = CString::new(name).expect("ELF name excludes NUL");
            let library = version.map(|v| CString::new(v.file).expect("ELF library excludes NUL"));
            let required = version.map(|v| CString::new(v.name).expect("ELF version excludes NUL"));
            let offset = view.symtab_entry_offset(index)?;
            let info = bytes[offset + 4]; // dynsym validated all 24 bytes above
            let flags = u32::from(info >> 4 == 2) | (u32::from(info & 0xf == 6) << 1);
            visitor.unwrap()(
                name.as_ptr(),
                library
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr()),
                required
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr()),
                flags,
                user,
            );
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            super::set_last_error(&format_args!("import inventory {path}: {error}"));
            -1
        }
    }
}
