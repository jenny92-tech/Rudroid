//! INPUT:  validated render geometry, immutable RGB precision policy and a live GLES 3 context
//! OUTPUT:  an owned low-resolution framebuffer, checked GL wrappers and scaled presentation
//! POS:    native graphics boundary restores queried guest state without redundant driver mutations
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
// Presentation shader adapted from Rudroid thunks/khronos/gles2.cpp.
#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_char, c_void, CStr};
use rudroid_common::display_dimensions::RenderDimensions;

pub(super) const FRAMEBUFFER: u32 = 0x8d40;
pub(super) const DRAW: u32 = 0x8ca9;
pub(super) const READ: u32 = 0x8ca8;
const DRAW_BINDING: u32 = 0x8ca6;
const READ_BINDING: u32 = 0x8caa;
const COLOR: u32 = 0x8ce0;
const BACK: u32 = 0x0405;
const TEXTURE: u32 = 0x0de1;

macro_rules! gl_table {
    ($($field:ident = $name:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub(super) struct Gl { $(pub $field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }
        impl Gl {
            fn load() -> Result<Self, String> {
                Ok(Self { $($field: {
                    // SAFETY: SDL resolves the declared GLES entry prototype; libraries remain pinned.
                    let address = unsafe { sdl2::sys::SDL_GL_GetProcAddress(concat!($name,"\0").as_ptr().cast()) };
                    if address.is_null() { return Err(concat!("render scaling requires ", $name).into()); }
                    // SAFETY: checked native entry with exactly the prototype declared above.
                    unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(address) }
                },)* })
            }
        }
    };
}
gl_table! {
    get = "glGetIntegerv": fn(u32,*mut i32);
    get64 = "glGetInteger64v": fn(u32,*mut i64);
    get_float = "glGetFloatv": fn(u32,*mut f32);
    get_bool = "glGetBooleanv": fn(u32,*mut u8);
    string = "glGetString": fn(u32) -> *const u8;
    error = "glGetError": fn() -> u32;
    bind = "glBindFramebuffer": fn(u32,u32);
    gen_framebuffer = "glGenFramebuffers": fn(i32,*mut u32);
    delete_framebuffer = "glDeleteFramebuffers": fn(i32,*const u32);
    is_framebuffer = "glIsFramebuffer": fn(u32) -> u8;
    status = "glCheckFramebufferStatus": fn(u32) -> u32;
    attach_texture = "glFramebufferTexture2D": fn(u32,u32,u32,u32,i32);
    attach_renderbuffer = "glFramebufferRenderbuffer": fn(u32,u32,u32,u32);
    attachment = "glGetFramebufferAttachmentParameteriv": fn(u32,u32,u32,*mut i32);
    gen_texture = "glGenTextures": fn(i32,*mut u32);
    delete_texture = "glDeleteTextures": fn(i32,*const u32);
    bind_texture = "glBindTexture": fn(u32,u32);
    texture_parameter = "glTexParameteri": fn(u32,u32,i32);
    texture_image = "glTexImage2D": fn(u32,i32,i32,i32,i32,i32,u32,u32,*const c_void);
    gen_renderbuffer = "glGenRenderbuffers": fn(i32,*mut u32);
    delete_renderbuffer = "glDeleteRenderbuffers": fn(i32,*const u32);
    bind_renderbuffer = "glBindRenderbuffer": fn(u32,u32);
    renderbuffer_storage = "glRenderbufferStorage": fn(u32,u32,i32,i32);
    bind_buffer = "glBindBuffer": fn(u32,u32);
    read_buffer = "glReadBuffer": fn(u32);
    draw_buffers = "glDrawBuffers": fn(i32,*const u32);
    invalidate = "glInvalidateFramebuffer": fn(u32,i32,*const u32);
    invalidate_sub = "glInvalidateSubFramebuffer": fn(u32,i32,*const u32,i32,i32,i32,i32);
    blit = "glBlitFramebuffer": fn(i32,i32,i32,i32,i32,i32,i32,i32,u32,u32);
    enabled = "glIsEnabled": fn(u32) -> u8;
    enable = "glEnable": fn(u32);
    disable = "glDisable": fn(u32);
    viewport = "glViewport": fn(i32,i32,i32,i32);
    scissor = "glScissor": fn(i32,i32,i32,i32);
    color_mask = "glColorMask": fn(u8,u8,u8,u8);
    create_shader = "glCreateShader": fn(u32) -> u32;
    shader_source = "glShaderSource": fn(u32,i32,*const *const c_char,*const i32);
    compile_shader = "glCompileShader": fn(u32);
    shader_status = "glGetShaderiv": fn(u32,u32,*mut i32);
    shader_log = "glGetShaderInfoLog": fn(u32,i32,*mut i32,*mut c_char);
    delete_shader = "glDeleteShader": fn(u32);
    create_program = "glCreateProgram": fn() -> u32;
    attach_shader = "glAttachShader": fn(u32,u32);
    link = "glLinkProgram": fn(u32);
    program_status = "glGetProgramiv": fn(u32,u32,*mut i32);
    program_log = "glGetProgramInfoLog": fn(u32,i32,*mut i32,*mut c_char);
    delete_program = "glDeleteProgram": fn(u32);
    uniform = "glGetUniformLocation": fn(u32,*const c_char) -> i32;
    use_program = "glUseProgram": fn(u32);
    uniform1i = "glUniform1i": fn(i32,i32);
    uniform2f = "glUniform2f": fn(i32,f32,f32);
    gen_vao = "glGenVertexArrays": fn(i32,*mut u32);
    delete_vao = "glDeleteVertexArrays": fn(i32,*const u32);
    bind_vao = "glBindVertexArray": fn(u32);
    active_texture = "glActiveTexture": fn(u32);
    bind_sampler = "glBindSampler": fn(u32,u32);
    draw = "glDrawArrays": fn(u32,i32,i32);
}

#[derive(Default)]
struct Objects { framebuffer:u32, texture:u32, depth:u32, program:u32, vao:u32 }
impl Objects {
    /// # Safety
    /// Owning primary context current, all guest calls stopped; these names belong only to us.
    unsafe fn delete(&self, gl:&Gl) {
        // SAFETY: caller guarantees owning current context and quiescent exclusive native ownership.
        unsafe {
            (gl.delete_program)(self.program); (gl.delete_vao)(1,&self.vao);
            (gl.delete_framebuffer)(1,&self.framebuffer); (gl.delete_texture)(1,&self.texture);
            (gl.delete_renderbuffer)(1,&self.depth);
        }
    }
}

pub(super) struct RenderScale { pub gl:Gl, objects:Objects, pub size:RenderDimensions, source:i32, texel:i32 }
impl RenderScale {
    /// # Safety
    /// Before guest execution on the SDL creator thread, primary context current and driver pinned.
    pub unsafe fn create(size:RenderDimensions) -> Result<Self,String> {
        let gl=Gl::load()?;
        // SAFETY: current creator-owned context; every output slot/attribute/source has valid storage.
        unsafe {
            let version=(gl.string)(0x1f02);
            if version.is_null() || !CStr::from_ptr(version.cast()).to_bytes().starts_with(b"OpenGL ES 3") {
                return Err("render scaling requires GLES 3; use 100% on GLES 2".into());
            }
            let mut texture_max=0; let mut renderbuffer_max=0;
            (gl.get)(0x0d33,&mut texture_max); (gl.get)(0x84e8,&mut renderbuffer_max);
            if size.width>texture_max.min(renderbuffer_max) || size.height>texture_max.min(renderbuffer_max) {
                return Err("render dimensions exceed native texture/renderbuffer limits".into());
            }
            let mut saved=[0;5];
            for (key,value) in [DRAW_BINDING,READ_BINDING,0x8069,0x8ca7,0x88ef].into_iter().zip(&mut saved) {(gl.get)(key,value);}
            let mut objects=Objects::default(); let mut source=-1;let mut texel=-1;
            let result=(|| -> Result<(),String> {
                (gl.gen_framebuffer)(1,&mut objects.framebuffer);(gl.bind)(FRAMEBUFFER,objects.framebuffer);
                (gl.gen_texture)(1,&mut objects.texture);(gl.bind_texture)(TEXTURE,objects.texture);
                for (key,value) in [(0x2801,0x2601),(0x2800,0x2601),(0x2802,0x812f),(0x2803,0x812f)] {(gl.texture_parameter)(TEXTURE,key,value);}
                (gl.bind_buffer)(0x88ec,0); // NULL is CPU null, never an inherited pixel-unpack-buffer offset.
                (gl.texture_image)(TEXTURE,0,0x8058,size.width,size.height,0,0x1908,0x1401,std::ptr::null());
                (gl.attach_texture)(FRAMEBUFFER,COLOR,TEXTURE,objects.texture,0);
                (gl.gen_renderbuffer)(1,&mut objects.depth);(gl.bind_renderbuffer)(0x8d41,objects.depth);
                (gl.renderbuffer_storage)(0x8d41,0x88f0,size.width,size.height);
                (gl.attach_renderbuffer)(FRAMEBUFFER,0x821a,0x8d41,objects.depth);
                if objects.framebuffer==0 || (gl.is_framebuffer)(objects.framebuffer)==0 || objects.texture==0 || objects.depth==0 || (gl.status)(FRAMEBUFFER)!=0x8cd5 {
                    return Err("scaled framebuffer allocation/integrity failed".into());
                }
                if size.sharp {
                    let half_precision=super::gles_policy::sharp_half_precision();
                    objects.program=program(&gl,half_precision)?;
                    eprintln!("[SDL] sharp presentation RGB precision={}",if half_precision {"mediump"}else{"highp"});
                    source=(gl.uniform)(objects.program,c"u_source".as_ptr());texel=(gl.uniform)(objects.program,c"u_texel".as_ptr());
                    (gl.gen_vao)(1,&mut objects.vao);
                    if source<0 || texel<0 || objects.vao==0 {return Err("scaled presentation uniforms/VAO unavailable".into());}
                }
                let error=(gl.error)();if error!=0 {return Err(format!("scaled initialization GL error {error:#x}"));}
                Ok(())
            })();
            (gl.bind_texture)(TEXTURE,saved[2] as u32);(gl.bind_renderbuffer)(0x8d41,saved[3] as u32);
            (gl.bind_buffer)(0x88ec,saved[4] as u32);(gl.bind)(DRAW,saved[0] as u32);(gl.bind)(READ,saved[1] as u32);
            if let Err(error)=result {objects.delete(&gl);return Err(error);}
            (gl.viewport)(0,0,size.width,size.height);(gl.scissor)(0,0,size.width,size.height);
            Ok(Self {gl,objects,size,source,texel})
        }
    }

    /// # Safety
    /// Owning primary context current; called before or after guest GL calls, without Rust locks.
    pub unsafe fn bind_defaults(&self) {
        // SAFETY: current owning context; scalar queries and binds do not borrow guest memory.
        unsafe {for (target,key) in [(DRAW,DRAW_BINDING),(READ,READ_BINDING)] {
            let mut value=0;(self.gl.get)(key,&mut value);
            if value==0 {(self.gl.bind)(target,self.objects.framebuffer);}
        }}
    }
    pub fn map_framebuffer(&self, name:u32) -> u32 {if name==0 {self.objects.framebuffer}else{name}}
    pub fn owns_framebuffer(&self,name:u32)->bool {name==self.objects.framebuffer}
    /// # Safety
    /// Current primary context; pname is any guest query. Returns only scalar overrides.
    pub unsafe fn override_integer(&self,pname:u32)->Option<i32> {
        // SAFETY: valid local scalar outputs and owning context, no guest pointers retained.
        unsafe {
            if matches!(pname,DRAW_BINDING|READ_BINDING) {
                let mut value=0;(self.gl.get)(pname,&mut value);
                return Some(if value as u32==self.objects.framebuffer {0}else{value});
            }
            if matches!(pname,0x0c02|0x8825) && self.is_default(if pname==0x0c02 {READ}else{DRAW}) {
                let mut value=0;(self.gl.get)(pname,&mut value);
                return Some(if value as u32==COLOR {BACK as i32}else{value});
            }
            None
        }
    }
    /// # Safety
    /// Current primary context; a GL target outside READ refers to DRAW, matching attachment APIs.
    pub unsafe fn is_default(&self,target:u32)->bool {
        let mut value=0;
        // SAFETY: current context and writable local scalar.
        unsafe {(self.gl.get)(if target==READ {READ_BINDING}else{DRAW_BINDING},&mut value);}
        value as u32==self.objects.framebuffer
    }
    /// # Safety
    /// Owning primary context is current on the rendering thread. No Rust lock spans native calls.
    pub unsafe fn present(&self) {
        let gl=&self.gl;
        // SAFETY: native state is local to the current context; all altered guest state is restored.
        unsafe {
            let mut saved=[0;2];(gl.get)(DRAW_BINDING,&mut saved[0]);
            // Sharp presentation draws from a texture and never changes the read framebuffer.
            if !self.size.sharp {(gl.get)(READ_BINDING,&mut saved[1]);}
            if self.size.sharp {self.present_sharp();} else {
                let scissor=(gl.enabled)(0x0c11);(gl.disable)(0x0c11);
                (gl.bind)(READ,self.objects.framebuffer);(gl.bind)(DRAW,0);
                (gl.blit)(0,0,self.size.width,self.size.height,0,0,self.size.output_width,self.size.output_height,0x4000,0x2601);
                if scissor!=0 {(gl.enable)(0x0c11);}
            }
            (gl.bind)(DRAW,saved[0] as u32);
            if !self.size.sharp {(gl.bind)(READ,saved[1] as u32);}
        }
    }
    /// # Safety
    /// Same context/lifetime contract as present; restores bindings, viewport, masks and enables.
    unsafe fn present_sharp(&self) {
        let gl=&self.gl;
        // SAFETY: current owning context; bounded local snapshots, no allocation or callback-held lock.
        unsafe {
            let mut saved=[0;3];for (key,value) in [0x8b8d,0x84e0,0x85b5].into_iter().zip(&mut saved) {(gl.get)(key,value);}
            let mut viewport=[0;4];(gl.get)(0x0ba2,viewport.as_mut_ptr());
            let mut mask=[0;4];(gl.get_bool)(0x0c23,mask.as_mut_ptr());
            let caps=[0x0be2,0x0b44,0x0b71,0x8c89,0x0c11,0x0b90];let enabled=caps.map(|cap|(gl.enabled)(cap));
            for (cap,on) in caps.into_iter().zip(enabled) {if on!=0 {(gl.disable)(cap);}}
            if mask!=[1;4] {(gl.color_mask)(1,1,1,1);}
            (gl.bind)(DRAW,0);(gl.viewport)(0,0,self.size.output_width,self.size.output_height);
            (gl.use_program)(self.objects.program);(gl.bind_vao)(self.objects.vao);
            if saved[1]!=0x84c0 {(gl.active_texture)(0x84c0);}
            let mut texture=0;let mut sampler=0;(gl.get)(0x8069,&mut texture);(gl.get)(0x8919,&mut sampler);
            if sampler!=0 {(gl.bind_sampler)(0,0);}
            (gl.bind_texture)(TEXTURE,self.objects.texture);
            (gl.uniform1i)(self.source,0);(gl.uniform2f)(self.texel,1.0/self.size.width as f32,1.0/self.size.height as f32);(gl.draw)(4,0,3);
            (gl.bind_texture)(TEXTURE,texture as u32);
            if sampler!=0 {(gl.bind_sampler)(0,sampler as u32);}
            if saved[1]!=0x84c0 {(gl.active_texture)(saved[1] as u32);}
            (gl.bind_vao)(saved[2] as u32);(gl.use_program)(saved[0] as u32);(gl.viewport)(viewport[0],viewport[1],viewport[2],viewport[3]);
            if mask!=[1;4] {(gl.color_mask)(mask[0],mask[1],mask[2],mask[3]);}
            for (cap,enabled) in caps.into_iter().zip(enabled) {if enabled!=0 {(gl.enable)(cap);}}
        }
    }
    /// # Safety
    /// Primary context current during quiescent shutdown, invoked exactly once by Graphics.
    pub unsafe fn destroy(&self) {
        // SAFETY: shutdown owns the current context and these private names.
        unsafe {self.objects.delete(&self.gl);}
    }
}

/// # Safety
/// Current creator-owned primary context, source is a live NUL-terminated static shader.
unsafe fn shader(gl:&Gl,kind:u32,source:&CStr)->Result<u32,String> {
    // SAFETY: bounded shader/source/output storage; a failed compile releases its only name.
    unsafe {
        let shader=(gl.create_shader)(kind);if shader==0 {return Err("upscaler shader allocation failed".into());}
        (gl.shader_source)(shader,1,&source.as_ptr(),std::ptr::null());(gl.compile_shader)(shader);
        let mut status=0;(gl.shader_status)(shader,0x8b81,&mut status);if status!=0 {return Ok(shader);}
        let mut log=[0u8;512];(gl.shader_log)(shader,512,std::ptr::null_mut(),log.as_mut_ptr().cast());(gl.delete_shader)(shader);
        Err(format!("upscaler shader failed: {}",String::from_utf8_lossy(&log)))
    }
}
// Only color arithmetic varies. Coordinates, the center sample and alpha stay highp.
// Both variants share the same five taps, clamp and strength; no additional pass or state.
fn sharp_fragment(half_precision:bool)->&'static CStr {
    macro_rules! source {
        ($color:literal) => {
            CStr::from_bytes_with_nul(concat!(
                "#version 300 es\nprecision highp float;\n",
                "uniform sampler2D u_source;uniform vec2 u_texel;in vec2 v_uv;out vec4 out_color;\n",
                "const ",$color," float sharp_strength=0.18;const ",$color," float neighbor_weight=4.0;\n",
                "void main(){vec4 s=texture(u_source,v_uv);",
                $color," vec3 c=s.rgb;",
                $color," vec3 n=texture(u_source,v_uv+vec2(0.0,u_texel.y)).rgb;",
                $color," vec3 b=texture(u_source,v_uv-vec2(0.0,u_texel.y)).rgb;",
                $color," vec3 e=texture(u_source,v_uv+vec2(u_texel.x,0.0)).rgb;",
                $color," vec3 w=texture(u_source,v_uv-vec2(u_texel.x,0.0)).rgb;",
                $color," vec3 lo=min(c,min(min(n,b),min(e,w)));",
                $color," vec3 hi=max(c,max(max(n,b),max(e,w)));",
                "out_color=vec4(clamp(c+(c*neighbor_weight-n-b-e-w)*sharp_strength,lo,hi),s.a);}\0"
            ).as_bytes()).expect("static NUL-terminated sharp shader")
        };
    }
    if half_precision {source!("mediump")}else{source!("highp")}
}

/// # Safety
/// Current creator-owned primary context; every partial native allocation is released on failure.
pub(super) unsafe fn program(gl:&Gl,half_precision:bool)->Result<u32,String> {
    // SAFETY: compile/link use static NUL-terminated sources and bounded local outputs.
    unsafe {
        let vertex=shader(gl,0x8b31,c"#version 300 es\nout vec2 v_uv;void main(){vec2 p=vec2(float((gl_VertexID<<1)&2),float(gl_VertexID&2));v_uv=p;gl_Position=vec4(p*2.0-1.0,0.0,1.0);}")?;
        let fragment=match shader(gl,0x8b30,sharp_fragment(half_precision)) {
            Ok(s)=>s,Err(e)=>{(gl.delete_shader)(vertex);return Err(e);}
        };
        let program=(gl.create_program)();
        if program!=0 {(gl.attach_shader)(program,vertex);(gl.attach_shader)(program,fragment);(gl.link)(program);}
        (gl.delete_shader)(vertex);(gl.delete_shader)(fragment);
        let mut status=0;if program!=0 {(gl.program_status)(program,0x8b82,&mut status);}
        if status!=0 {return Ok(program);}
        let mut log=[0u8;512];if program!=0 {(gl.program_log)(program,512,std::ptr::null_mut(),log.as_mut_ptr().cast());(gl.delete_program)(program);}
        Err(format!("upscaler link failed: {}",String::from_utf8_lossy(&log)))
    }
}
