//! INPUT:  live Graphics ownership, current native GL bindings and guest API arguments
//! OUTPUT:  default-framebuffer redirection, consistent queries and translated attachment hints
//! POS:    native ABI wrappers query context only for virtualized state; no guest pointer retention
#![deny(unsafe_op_in_unsafe_fn)]
use super::active;
use super::super::render_scale::{DRAW,READ};

pub(super) fn symbol(name:&[u8])->usize {
    match name {
        b"glBindFramebuffer"|b"glBindFramebufferOES"|b"glBindFramebufferEXT"=>bind as usize,
        b"glGetIntegerv"=>get_integer as usize,b"glGetInteger64v"=>get_integer64 as usize,
        b"glGetFloatv"=>get_float as usize,b"glGetBooleanv"=>get_boolean as usize,
        b"glReadBuffer"=>read_buffer as usize,b"glDrawBuffers"=>draw_buffers as usize,
        b"glInvalidateFramebuffer"|b"glDiscardFramebufferEXT"=>invalidate as usize,
        b"glInvalidateSubFramebuffer"=>invalidate_sub as usize,
        b"glDeleteFramebuffers"|b"glDeleteFramebuffersOES"|b"glDeleteFramebuffersEXT"=>delete_framebuffers as usize,
        b"glGetFramebufferAttachmentParameteriv"|b"glGetFramebufferAttachmentParameterivOES"|b"glGetFramebufferAttachmentParameterivEXT"=>attachment as usize,
        _=>0,
    }
}

/// Guest pointers below follow their GL prototype: readable/writable for the declared count.
/// Every wrapper owns an Arc for the whole native call; only the owning primary context redirects.
unsafe extern "C" fn bind(target:u32,name:u32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    // Only logical framebuffer zero depends on the current primary binding.
    // Nonzero names must pass through identically in primary and worker contexts.
    let name=if name==0 {g.primary_scale().map_or(name,|s|s.map_framebuffer(name))}else{name};
    // SAFETY: pinned entry, same GL prototype/current-context contract as the caller.
    unsafe {(scale.gl.bind)(target,name);}
}

macro_rules! getter {
    ($function:ident,$entry:ident,$ty:ty,$convert:expr)=>{
        unsafe extern "C" fn $function(pname:u32,out:*mut $ty) {
            let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
            // SAFETY: out is writable according to this GL query's caller contract; never stored.
            unsafe {
                // These are the only scalar queries virtualized by RenderScale.
                // Other queries keep their native output length and need no EGL binding queries.
                if !out.is_null() && matches!(pname,0x8ca6|0x8caa|0x0c02|0x8825) {
                    if let Some(value)=g.primary_scale().and_then(|s|s.override_integer(pname)) {
                        *out=($convert)(value);return;
                    }
                }
                (scale.gl.$entry)(pname,out);
            }
        }
    }
}
getter!(get_integer,get,i32,|v:i32|v);
getter!(get_integer64,get64,i64,|v:i32|i64::from(v));
getter!(get_float,get_float,f32,|v:i32|v as f32);
getter!(get_boolean,get_bool,u8,|v:i32|u8::from(v!=0));

unsafe extern "C" fn read_buffer(source:u32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    // SAFETY: owning context checked before binding queries; native entry uses a scalar enum only.
    unsafe {
        let source=if source==0x0405 && g.primary_scale().is_some_and(|s|s.is_default(READ)) {0x8ce0}else{source};
        (scale.gl.read_buffer)(source);
    }
}

enum ArrayKind { Draw, Attachments }
/// # Safety
/// For a positive count, source points to count readable u32 values. Returned temporary storage
/// stays borrowed until the synchronous native call; nothing is stored in Graphics or a callback.
unsafe fn mapped_array(count:i32,source:*const u32,kind:ArrayKind,stack:&mut [u32;32],heap:&mut Vec<u32>)->*const u32 {
    if count<=0 || source.is_null() {return source;}
    let count=count as usize;
    let target=if count<=stack.len() {&mut stack[..count]}else{
        // Rare oversized hint lists use a fallible allocation; normal frame hints never allocate.
        if heap.try_reserve_exact(count).is_err() {return source;}
        heap.resize(count,0);heap.as_mut_slice()
    };
    // SAFETY: caller's positive-count GL array contract; stack/heap have exactly count output slots.
    let input=unsafe {std::slice::from_raw_parts(source,count)};
    for (out,value) in target.iter_mut().zip(input) {*out=match (&kind,*value) {
        (ArrayKind::Draw,0x0405)|(ArrayKind::Attachments,0x1800)=>0x8ce0,
        (ArrayKind::Attachments,0x1801)=>0x8d00,(ArrayKind::Attachments,0x1802)=>0x8d20,
        _=>*value,
    };}
    target.as_ptr()
}
unsafe extern "C" fn draw_buffers(count:i32,buffers:*const u32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    let mut stack=[0;32];let mut heap=Vec::new();
    // SAFETY: source obeys the GL array contract and mapped storage spans the synchronous native call.
    unsafe {
        let buffers=if g.primary_scale().is_some_and(|s|s.is_default(DRAW)) {
            mapped_array(count,buffers,ArrayKind::Draw,&mut stack,&mut heap)
        }else{buffers};
        (scale.gl.draw_buffers)(count,buffers);
    }
}
unsafe extern "C" fn invalidate(target:u32,count:i32,attachments:*const u32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    let mut stack=[0;32];let mut heap=Vec::new();
    // SAFETY: borrowed GL attachment array and checked owning default target; no pointer escapes.
    unsafe {
        let attachments=if g.primary_scale().is_some_and(|s|s.is_default(target)) {
            mapped_array(count,attachments,ArrayKind::Attachments,&mut stack,&mut heap)
        }else{attachments};
        (scale.gl.invalidate)(target,count,attachments);
    }
}
unsafe extern "C" fn invalidate_sub(target:u32,count:i32,attachments:*const u32,x:i32,y:i32,w:i32,h:i32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    let mut stack=[0;32];let mut heap=Vec::new();
    // SAFETY: same borrowed attachment contract; rectangle is forwarded unchanged for native validation.
    unsafe {
        let attachments=if g.primary_scale().is_some_and(|s|s.is_default(target)) {
            mapped_array(count,attachments,ArrayKind::Attachments,&mut stack,&mut heap)
        }else{attachments};
        (scale.gl.invalidate_sub)(target,count,attachments,x,y,w,h);
    }
}
unsafe extern "C" fn delete_framebuffers(count:i32,names:*const u32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    // SAFETY: positive GL count owns count readable names; each synchronous call borrows one scalar.
    unsafe {
        if g.primary_scale().is_none() || count<=0 || names.is_null() {
            // Framebuffer names are context-local: worker name 1 may be legitimate even when our private name is 1.
            (scale.gl.delete_framebuffer)(count,names);return;
        }
        for name in std::slice::from_raw_parts(names,count as usize) {
            if !scale.owns_framebuffer(*name) {(scale.gl.delete_framebuffer)(1,name);}
        }
        // Native deletion of a bound guest FBO resets its binding to zero; restore the logical default.
        if let Some(scale)=g.primary_scale() {scale.bind_defaults();}
    }
}
unsafe extern "C" fn attachment(target:u32,attachment:u32,pname:u32,out:*mut i32) {
    let Some(g)=active() else{return;};let Some(scale)=g.scale.as_ref() else{return;};
    // SAFETY: caller owns this writable scalar. Current default-target queries never leak private names.
    unsafe {
        let default=g.primary_scale().is_some_and(|s|s.is_default(target));
        if default && !out.is_null() && matches!(pname,0x8cd0|0x8cd1) {
            *out=if pname==0x8cd0 {0x8218}else{0};return;
        }
        let attachment=if default {match attachment {0x0405|0x1800=>0x8ce0,0x1801=>0x8d00,0x1802=>0x8d20,_=>attachment}}else{attachment};
        (scale.gl.attachment)(target,attachment,pname,out);
    }
}
