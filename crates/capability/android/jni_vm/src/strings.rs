//! INPUT: Java UTF-16 units and modified UTF-8 string entities
//! OUTPUT: lazy immutable UTF-16 views, correctly bounded regions and paired borrows
//! POS: alternate encoding cache inside the same shared JNI string owner
use super::{abi,RefTarget,Session};
use std::ffi::{c_char,c_void};
use std::sync::OnceLock;

pub(super) struct StringData { bytes:Box<[u8]>, utf16:OnceLock<Box<[u16]>> }
impl StringData {
    pub(super) fn new(bytes:Box<[u8]>)->Self {Self{bytes,utf16:OnceLock::new()}}
    fn units(&self)->&[u16] {
        self.utf16.get_or_init(|| {
            let mut units=Vec::with_capacity(self.bytes.len());let mut i=0;
            while i+1<self.bytes.len() {
                let b=self.bytes[i];
                let (unit,n)=if b<0x80 {(u16::from(b),1)}
                    else if b&0xe0==0xc0 && i+2<self.bytes.len() {(((u16::from(b)&31)<<6)|(u16::from(self.bytes[i+1])&63),2)}
                    else if b&0xf0==0xe0 && i+3<self.bytes.len() {(((u16::from(b)&15)<<12)|((u16::from(self.bytes[i+1])&63)<<6)|(u16::from(self.bytes[i+2])&63),3)}
                    else {(0xfffd,1)};
                units.push(unit);i+=n;
            }
            units.push(0);units.into_boxed_slice()
        })
    }
}
impl std::ops::Deref for StringData {type Target=[u8];fn deref(&self)->&[u8]{&self.bytes}}
fn encode(units:&[u16])->Vec<u8> {
    let mut bytes=Vec::with_capacity(units.len().saturating_mul(3));
    for &unit in units {
        if (1..=127).contains(&unit) {bytes.push(unit as u8);}
        else if unit<0x800 {bytes.extend_from_slice(&[0xc0|(unit>>6) as u8,0x80|(unit&63) as u8]);}
        else {bytes.extend_from_slice(&[0xe0|(unit>>12) as u8,0x80|((unit>>6)&63) as u8,0x80|(unit&63) as u8]);}
    }
    bytes
}
impl Session {
    /// Read immutable string storage under its owner lock. The callback must
    /// not re-enter the session; its borrow cannot escape through `R`.
    fn with_string_data<R>(&self, string: *mut c_void, units: bool, read: impl FnOnce(&StringData) -> R) -> Option<R> {
        if units && self.blocked_by_pending("GetStringChars") { return None; }
        let Some(RefTarget::Str(id)) = self.resolve(string) else {
            if units {
                self.set_pending("java/lang/IllegalArgumentException", "not a string");
            } else {
                self.log("GetStringUTFChars: not a live string reference");
                self.set_pending("java/lang/StringIndexOutOfBoundsException", "not a live string");
            }
            return None;
        };
        let strings = self.strings.lock().unwrap_or_else(|p| p.into_inner());
        strings.get(&id).map(read)
    }

    pub(super) fn with_string_bytes<R>(&self, string: *mut c_void, read: impl FnOnce(&[u8]) -> R) -> Option<R> {
        self.with_string_data(string, false, |data| {
            // Match GetStringUTFChars/CStr even for internal byte strings
            // containing a literal NUL; encoded Java NUL remains C0 80.
            let end = data.iter().position(|&byte| byte == 0).unwrap_or(data.len());
            read(&data[..end])
        })
    }

    pub(super) fn with_string_units<R>(&self, string: *mut c_void, read: impl FnOnce(&[u16]) -> R) -> Option<R> {
        self.with_string_data(string, true, |data| {
            let units = data.units();
            read(&units[..units.len() - 1])
        })
    }

    pub(super) fn new_string_units(&self, units: &[u16]) -> *mut c_void {
        self.new_string(&encode(units))
    }

    pub(super) fn string_units_length(&self,string:*mut c_void)->i32 {
        if self.blocked_by_pending("GetStringLength"){return 0;}
        let Some(RefTarget::Str(id))=self.resolve(string) else {self.set_pending("java/lang/IllegalArgumentException","not a string");return 0;};
        self.strings.lock().unwrap().get(&id).map(|s|(s.units().len()-1) as i32).unwrap_or(0)
    }
    pub(super) fn borrow_string_units(&self,string:*mut c_void)->*const u16 {
        self.borrow_string_data(string, true).cast()
    }

    /// One ledger/lock order for UTF-8, UTF-16 and critical UTF-16 pins.
    /// Reserve/check every fallible operation before publishing the owner pin.
    pub(super) fn borrow_string_data(&self, string:*mut c_void, utf16:bool)->*const c_void {
        let entry=if utf16 {"GetStringChars"} else {"GetStringUTFChars"};
        if self.blocked_by_pending(entry){return std::ptr::null();}
        let Some(RefTarget::Str(id))=self.resolve(string) else {
            self.set_pending(if utf16 {"java/lang/IllegalArgumentException"} else {"java/lang/StringIndexOutOfBoundsException"},"not a live string");
            return std::ptr::null();
        };
        // Borrow table -> liveness -> entity storage; no callbacks under these locks.
        let mut borrows=self.string_chars.lock().unwrap_or_else(|p|p.into_inner());
        let mut live=self.liveness.lock().unwrap_or_else(|p|p.into_inner());
        let strings=self.strings.lock().unwrap_or_else(|p|p.into_inner());
        let Some(data)=strings.get(&id) else{return std::ptr::null();};
        let pointer=if utf16 {data.units().as_ptr().cast::<c_void>()} else {data.as_ptr().cast::<c_void>()};
        let key=pointer as usize;
        let pins=live.get(&id).copied().and_then(|n|n.checked_add(1));
        let leases=borrows.get(&key).map_or(Some(1),|(_,n)|n.checked_add(1));
        if pins.is_none() || leases.is_none() || (!borrows.contains_key(&key) && borrows.try_reserve(1).is_err()) {
            drop((strings,live,borrows));
            self.set_pending("java/lang/OutOfMemoryError","string borrow accounting exhausted");
            return std::ptr::null();
        }
        *live.get_mut(&id).expect("validated string owner")=pins.unwrap();
        borrows.insert(key,(id,leases.unwrap()));
        pointer
    }
}
pub(super) mod shims {
    use super::*;
    pub extern "C" fn string_bytes_utf8(_env:*mut c_void,string:*mut c_void,encoding:*mut c_void)->*mut c_void{
        let Some(s)=super::super::session() else{return std::ptr::null_mut();};
        if s.blocked_by_pending("String.getBytes"){return std::ptr::null_mut();}
        let Some(RefTarget::Str(id))=s.resolve(string) else{s.set_pending("java/lang/IllegalArgumentException","not a string");return std::ptr::null_mut();};
        let Some(RefTarget::Str(encoding))=s.resolve(encoding) else{s.set_pending("java/lang/NullPointerException","encoding is null");return std::ptr::null_mut();};
        let bytes={let strings=s.strings.lock().unwrap();
            let Some(name)=strings.get(&encoding) else{return std::ptr::null_mut();};
            if !matches!(&name[..name.len()-1],b"UTF-8"|b"UTF8"|b"utf-8"|b"utf8"){
                drop(strings);s.set_pending("java/io/UnsupportedEncodingException","only UTF-8 encoding is available");return std::ptr::null_mut();
            }
            let value=&strings[&id];let units=value.units();
            char::decode_utf16(units[..units.len()-1].iter().copied()).map(|c|c.unwrap_or('?')).collect::<String>().into_bytes()
        };
        let result=s.new_byte_array(bytes.len() as i32);
        eprintln!("[JNI] String.getBytes bytes={}",bytes.len());
        if let Some(RefTarget::Array(id)) = s.resolve(result) {
            let mut arrays = s.arrays.lock().unwrap();
            if let Some(output) = arrays.get_mut(&id).and_then(|array| array.primitive.as_mut().and_then(super::super::primitive_arrays::Values::bytes_mut)) {
                for (out, byte) in output.iter_mut().zip(bytes) { *out = byte as i8; }
            }
        }
        result
    }
    pub unsafe extern "C" fn new_string(_env:*mut c_void,chars:*const u16,length:i32)->*mut c_void {
        let Some(s)=super::super::session() else{return std::ptr::null_mut();};
        if length<0 || length>0&&chars.is_null(){s.set_pending("java/lang/IllegalArgumentException","invalid UTF-16 input");return std::ptr::null_mut();}
        // SAFETY: the JNI caller supplies `length` readable UTF-16 units; zero accepts null.
        let units=if length==0{&[]}else{unsafe { std::slice::from_raw_parts(chars,length as usize) }};
        s.new_string_units(units)
    }
    pub extern "C" fn get_string_length(_env:*mut c_void,string:*mut c_void)->i32 {super::super::session().map(|s|s.string_units_length(string)).unwrap_or(0)}
    pub unsafe extern "C" fn get_string_chars(_env:*mut c_void,string:*mut c_void,copy:*mut u8)->*const u16 {
        if !copy.is_null(){*copy=abi::JNI_FALSE;}
        super::super::session().map(|s|s.borrow_string_units(string)).unwrap_or(std::ptr::null())
    }
    /// # Safety
    /// Release only an owned, outstanding JNI string borrow, with no users
    /// of that borrow remaining. A matching pointer alone does not own a lease.
    pub unsafe extern "C" fn release_string_chars(_env:*mut c_void,_string:*mut c_void,chars:*const u16) {if let Some(s)=super::super::session(){s.release_string_chars(chars.cast::<c_char>());}}
    pub unsafe extern "C" fn get_string_region(_env:*mut c_void,string:*mut c_void,start:i32,length:i32,buffer:*mut u16) {region(string,start,length,buffer.cast(),false);}
    pub unsafe extern "C" fn get_string_utf_region(_env:*mut c_void,string:*mut c_void,start:i32,length:i32,buffer:*mut c_char) {region(string,start,length,buffer.cast(),true);}
    unsafe fn region(string:*mut c_void,start:i32,length:i32,buffer:*mut c_void,utf8:bool) {
        let Some(s)=super::super::session() else{return;};if s.blocked_by_pending("GetStringRegion"){return;}
        let Some(RefTarget::Str(id))=s.resolve(string) else{s.set_pending("java/lang/IllegalArgumentException","not a string");return;};
        let strings=s.strings.lock().unwrap();let Some(string)=strings.get(&id) else{return;};let units=string.units();
        let range=usize::try_from(start).ok().zip(usize::try_from(length).ok()).and_then(|(start,length)|start.checked_add(length).filter(|end|*end<units.len()).map(|end|start..end));
        let Some(range)=range else{drop(strings);s.set_pending("java/lang/StringIndexOutOfBoundsException","UTF-16 region out of bounds");return;};
        if !range.is_empty()&&buffer.is_null(){drop(strings);s.set_pending("java/lang/IllegalArgumentException","null string region output");return;}
        if utf8 {let bytes=encode(&units[range]);if !bytes.is_empty(){std::ptr::copy_nonoverlapping(bytes.as_ptr(),buffer.cast(),bytes.len());}}
        else if !range.is_empty(){std::ptr::copy_nonoverlapping(units[range.clone()].as_ptr(),buffer.cast::<u16>(),range.len());}
    }
}
