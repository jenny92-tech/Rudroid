//! INPUT: Java FileInputStream paths, shared JNI arrays and loader buffer policy
//! OUTPUT: one File per stream, budgeted read-ahead and automatic buffer/fd release
//! POS: core Java stream payload in the authoritative object heap; no raw Rust owner crosses FFI
#![forbid(unsafe_code)]
use super::RefTarget;
use std::ffi::c_void;
use std::io::Read;
use std::sync::Arc;
use super::stream_buffer::{BufferBudget, FileStream};

pub(super) mod shims {
    use super::*;
    pub extern "C" fn stream_init(_env:*mut c_void,object:*mut c_void,path:*mut c_void) {
        let Some(s)=super::super::session() else{return;};let Some(RefTarget::Object(id))=s.resolve(object) else{return;};
        let Some(path) = s.with_string_units(path, String::from_utf16) else { return; };
        let path=path.ok().and_then(|path| {
            let Some(host)=s.host.get() else{return Some(path);};
            let native=std::ffi::CString::new(path.as_bytes()).ok()?;
            match host.asset_path(rudroid_common::android_assets::PATH,&native,None,0) {
                Ok(Some(resolved))=>Some(resolved.to_string_lossy().into_owned()),Ok(None)=>Some(path),Err(_)=>None,
            }
        });
        let opened=path.map(|path| std::fs::File::open(&path).map(|file| (file, path)));
        match opened {Some(Ok((file, path)))=>{
            let stream = FileStream::new(file, std::path::Path::new(&path), s.stream_buffers.get_or_init(|| Arc::new(
                s.host.get().map(BufferBudget::from_host).unwrap_or(BufferBudget::disabled())
            )));
            s.objects.lock().unwrap().get_mut(&id).unwrap().stream=Some(Box::new(stream));
        },Some(Err(error))=>s.set_pending("java/io/FileNotFoundException",&error.to_string()),None=>s.set_pending("java/io/FileNotFoundException","invalid UTF-8 path")}
    }
    pub extern "C" fn stream_close(_env:*mut c_void,object:*mut c_void) {
        if let Some(s)=super::super::session(){if let Some(RefTarget::Object(id))=s.resolve(object){s.objects.lock().unwrap().get_mut(&id).unwrap().stream=None;}}
    }
    pub extern "C" fn stream_read(env:*mut c_void,object:*mut c_void,array:*mut c_void)->i32 {
        let Some(s)=super::super::session() else{return -1;};let length=s.array_length(array);
        if s.blocked_by_pending("InputStream.read"){return -1;}stream_read_range(env,object,array,0,length)
    }
    pub extern "C" fn stream_read_range(_env:*mut c_void,object:*mut c_void,array:*mut c_void,start:i32,length:i32)->i32 {
        let Some(s)=super::super::session() else{return -1;};let(Some(RefTarget::Object(object)),Some(RefTarget::Array(array)))=(s.resolve(object),s.resolve(array)) else {s.set_pending("java/lang/IllegalArgumentException","invalid stream or array");return -1;};
        let mut objects=s.objects.lock().unwrap();let mut arrays=s.arrays.lock().unwrap();
        let Some(file)=objects.get_mut(&object).and_then(|o|o.stream.as_mut()) else {drop(arrays);drop(objects);s.set_pending("java/io/IOException","stream is closed");return -1;};
        let Some(bytes)=arrays.get_mut(&array).and_then(|a|a.primitive.as_mut().and_then(super::super::primitive_arrays::Values::bytes_mut)) else {drop(arrays);drop(objects);s.set_pending("java/lang/IllegalArgumentException","not a byte array");return -1;};
        let range=usize::try_from(start).ok().zip(usize::try_from(length).ok()).and_then(|(start,length)|start.checked_add(length).filter(|end|*end<=bytes.len()).map(|end|start..end));
        let Some(range)=range else {drop(arrays);drop(objects);s.set_pending("java/lang/IndexOutOfBoundsException","stream byte range out of bounds");return -1;};if range.is_empty(){return 0;}
        let buffer=super::super::bytes::unsigned_mut(&mut bytes[range]);
        let result=file.read(buffer);drop(arrays);drop(objects);
        match result {Ok(0)=>-1,Ok(count)=>count as i32,Err(error)=>{s.set_pending("java/io/IOException",&error.to_string());-1}}
    }
    pub extern "C" fn stream_available(_env:*mut c_void,object:*mut c_void)->i32 {
        let Some(s)=super::super::session() else{return 0;};let Some(RefTarget::Object(id))=s.resolve(object) else{return 0;};
        let mut objects=s.objects.lock().unwrap();let available=objects.get_mut(&id).and_then(|o|o.stream.as_mut()).and_then(|f|f.metadata().ok().zip(f.stream_position().ok())).map(|(meta,pos)|meta.len().saturating_sub(pos).min(i32::MAX as u64) as i32);drop(objects);
        available.unwrap_or_else(||{s.set_pending("java/io/IOException","stream is closed");0})
    }
}

pub(super) mod scanner {
    use super::*;
    use crate::session::{fields::FieldValue,make_handle,TAG_FIELD};
    fn source(s:&super::super::Session,object:*mut c_void)->Option<RefTarget> {
        let class=s.object_class(object,"Scanner")?;
        let field=s.define_field(class,"source","Ljava/io/InputStream;",false)?;
        let FieldValue::Object(target)=s.get_field(object,make_handle(TAG_FIELD,field),FieldValue::Object(None),false)? else{return None;};target
    }
    pub extern "C" fn init(_env:*mut c_void,object:*mut c_void,stream:*mut c_void,encoding:*mut c_void) {
        let Some(s)=super::super::session() else{return;};
        let Some(utf8)=s.with_string_bytes(encoding, |bytes| matches!(bytes,b"UTF-8"|b"UTF8")) else {return;};
        if !utf8{s.set_pending("java/io/UnsupportedEncodingException","Scanner supports UTF-8");return;}
        let Some(class)=s.object_class(object,"Scanner") else{return;};let Some(field)=s.define_field(class,"source","Ljava/io/InputStream;",false) else{return;};
        s.set_field(object,make_handle(TAG_FIELD,field),FieldValue::Object(s.resolve(stream)),false);
    }
    pub extern "C" fn delimiter(_env:*mut c_void,object:*mut c_void,delimiter:*mut c_void)->*mut c_void {
        let Some(s)=super::super::session() else{return std::ptr::null_mut();};
        // Java's absolute end anchor is another whole-stream delimiter. Unity 6
        // uses \z while older players use \A; rejecting \z discards boot.config.
        // Keep other regexes unavailable rather than pretending to implement them.
        let Some(all)=s.with_string_bytes(delimiter, |bytes| matches!(bytes, b"\\A" | b"\\z")) else {return std::ptr::null_mut();};
        if !all{s.set_pending("java/lang/UnsupportedOperationException","Scanner only supports the whole-stream delimiter");return std::ptr::null_mut();}
        s.new_ref(s.resolve(object).unwrap(),false)
    }
    pub extern "C" fn next(_env:*mut c_void,object:*mut c_void)->*mut c_void {
        let Some(s)=super::super::session() else{return std::ptr::null_mut();};let Some(target@RefTarget::Object(id))=source(s,object) else{return std::ptr::null_mut();};
        let result={let mut objects=s.objects.lock().unwrap();let stream=objects.get_mut(&id).and_then(|o|o.stream.as_mut());
            if let Some(stream)=stream{let mut data=String::new();stream.read_to_string(&mut data).map(|_|data)}else{Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe,"stream is closed"))}};
        s.release_target(target);
        match result {Ok(data)=>{let units:Vec<u16>=data.encode_utf16().collect();s.new_string_units(&units)},Err(error)=>{s.set_pending("java/io/IOException",&error.to_string());std::ptr::null_mut()}}
    }
    pub extern "C" fn has_next(_env:*mut c_void,object:*mut c_void)->u8 {
        let Some(s)=super::super::session() else{return 0;};let Some(target@RefTarget::Object(id))=source(s,object) else{return 0;};
        let available={let mut objects=s.objects.lock().unwrap();objects.get_mut(&id).and_then(|o|o.stream.as_mut()).and_then(|f|f.metadata().ok().zip(f.stream_position().ok())).is_some_and(|(meta,pos)|meta.len()>pos)};
        s.release_target(target);u8::from(available)
    }
    pub extern "C" fn close(env:*mut c_void,object:*mut c_void) {
        let Some(s)=super::super::session() else{return;};if let Some(target)=source(s,object){let local=s.new_ref(target,false);shims::stream_close(env,local);s.delete_ref(local,false,"Scanner.close");s.release_target(target);}
    }
}
