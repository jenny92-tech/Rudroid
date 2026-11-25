//! INPUT: opt-in capture settings and a current primary GLES context
//! OUTPUT: bounded diagnostic frame readbacks
//! POS: private SDL renderer probe enabled by diag-probes
use std::{ffi::{c_char, c_void, CStr}, sync::atomic::{AtomicU32, Ordering}};
use super::Graphics;
/// Opt-in diagnostic readback. Normal sessions have no capture allocation or
/// pixel readback; the environment is queried once at service activation.
#[derive(Clone, Copy)]
enum CapturePoint { Swap(u64), ElapsedMs(u64) }
pub(super) struct FrameCapture {
    path: std::path::PathBuf,
    points: Vec<CapturePoint>, // primary plus at most 16 extra captures
    started: Option<std::time::Instant>,
    completed: AtomicU32,
}
impl FrameCapture {
    pub(super) fn from_environment() -> Option<Self> {
        if !rudroid_common::logging::diagnostics_enabled() { return None; }
        let path = std::env::var_os("RUDROID_CAPTURE_GUEST_FRAME_PATH")?;
        let after_ms = std::env::var("RUDROID_CAPTURE_GUEST_FRAME_AFTER_MS").ok().and_then(|s|s.parse::<u64>().ok());
        let at = after_ms.unwrap_or_else(|| std::env::var("RUDROID_CAPTURE_GUEST_FRAME_AT").ok().and_then(|s|s.parse().ok()).unwrap_or(120));
        let (point, extra_key): (fn(u64) -> CapturePoint, _) = if after_ms.is_some() {
            (CapturePoint::ElapsedMs, "RUDROID_CAPTURE_GUEST_FRAME_EXTRA_AFTER_MS")
        } else {
            (CapturePoint::Swap, "RUDROID_CAPTURE_GUEST_FRAME_EXTRA")
        };
        let mut points=vec![point(at)];
        points.extend(std::env::var(extra_key).unwrap_or_default().split(',').filter_map(|s|s.parse::<u64>().ok()).filter(|n|*n>0).take(16).map(point));
        Some(Self { path:path.into(), points, started:after_ms.map(|_|std::time::Instant::now()), completed:AtomicU32::new(0) })
    }
    fn take_due(&self, swap:u64, elapsed_ms:u128)->Option<usize> {
        self.points.iter().enumerate().find_map(|(index,point)| {
            let due=match point { CapturePoint::Swap(at)=>swap==*at, CapturePoint::ElapsedMs(at)=>elapsed_ms>=u128::from(*at) };
            let bit=1u32<<index;
            (due && self.completed.fetch_or(bit, Ordering::Relaxed)&bit==0).then_some(index)
        })
    }
    pub(super) fn take(&self,swap:u64)->Option<usize> {
        self.take_due(swap,self.started.map(|start|start.elapsed().as_millis()).unwrap_or(0))
    }
    pub(super) unsafe fn read(&self, g:&Graphics) -> Option<Vec<u8>> {
        if !g.primary_current(){return None;}
        let version=g.native_address(c"glGetString");if version==0{return None;}
        let version:unsafe extern "C" fn(u32)->*const c_char=std::mem::transmute(version);
        let version=version(0x1f02);if version.is_null() || !CStr::from_ptr(version).to_bytes().starts_with(b"OpenGL ES 3"){return None;}
        let names = [c"glGetIntegerv",c"glBindFramebuffer",c"glBindBuffer",c"glPixelStorei",c"glReadPixels",c"glReadBuffer"];
        let addresses = names.map(|name|g.resolve_gl(name));
        if addresses.contains(&0) {return None;}
        let get:unsafe extern "C" fn(u32,*mut i32)=std::mem::transmute(addresses[0]);
        let framebuffer:unsafe extern "C" fn(u32,u32)=std::mem::transmute(addresses[1]);
        let buffer:unsafe extern "C" fn(u32,u32)=std::mem::transmute(addresses[2]);
        let pack:unsafe extern "C" fn(u32,i32)=std::mem::transmute(addresses[3]);
        let read:unsafe extern "C" fn(i32,i32,i32,i32,u32,u32,*mut c_void)=std::mem::transmute(addresses[4]);
        let read_buffer:unsafe extern "C" fn(u32)=std::mem::transmute(addresses[5]);
        // Diagnostic checks use the native entry. Replay any pre-existing
        // error through the guest façade, keyed by context rather than thread.
        let error_address=g.native_address(c"glGetError");
        if error_address==0{return None;}
        let error:unsafe extern "C" fn()->u32=std::mem::transmute(error_address);
        let context=(g.driver.current_context)() as usize;
        if g.capture_errors.lock().unwrap_or_else(|p|p.into_inner()).contains_key(&context){return None;}
        let prior=error();if prior!=0{g.remember_gl_error(context,prior);rudroid_common::error!("[SDL] guest capture skipped: pre-existing GL error {prior:#x}");return None;}
        // ES3 state restoration is required: a bound pixel-pack buffer would
        // reinterpret the CPU output pointer as a buffer offset, and row/skip
        // parameters could otherwise write beyond the owned output allocation.
        let mut saved=[0;6];
        let keys=[0x8caa,0x88ed,0x0d05,0x0d02,0x0d03,0x0d04];
        for (key,value) in keys.into_iter().zip(&mut saved){get(key,value);}
        let Some(size)=(g.width as usize).checked_mul(g.height as usize).and_then(|n|n.checked_mul(4)) else{return None;};
        let mut pixels=Vec::new();if pixels.try_reserve_exact(size).is_err(){return None;}pixels.resize(size,0u8);
        framebuffer(0x8ca8,0);let mut old_read_buffer=0;get(0x0c02,&mut old_read_buffer);read_buffer(0x0405);buffer(0x88eb,0);pack(0x0d05,1);
        for key in [0x0d02,0x0d03,0x0d04]{pack(key,0);}
        read(0,0,g.width,g.height,0x1908,0x1401,pixels.as_mut_ptr().cast());
        let read_error=error();read_buffer(old_read_buffer as u32);
        framebuffer(0x8ca8,saved[0] as u32);buffer(0x88eb,saved[1] as u32);
        for (key,value) in keys[2..].iter().zip(&saved[2..]){pack(*key,*value);}
        let restore_error=error();if read_error!=0 || restore_error!=0 {rudroid_common::error!("[SDL] guest capture failed: read={read_error:#x} restore={restore_error:#x}");return None;}
        Some(pixels)
    }
    pub(super) fn write(&self,g:&Graphics,pixels:&[u8],at:u64,index:usize) {
        use std::io::Write;
        let result=(||->std::io::Result<()>{
            let path=if index==0{self.path.clone()}else{
                let suffix=match self.points[index] {CapturePoint::Swap(n)=>format!("swap-{n}"),CapturePoint::ElapsedMs(n)=>format!("ms-{n}")};
                self.path.with_file_name(format!("{}-{suffix}.ppm",self.path.file_stem().unwrap_or_default().to_string_lossy()))
            };
            let mut file=std::io::BufWriter::new(std::fs::File::create(path)?);
            write!(file,"P6\n{} {}\n255\n",g.width,g.height)?;
            for row in (0..g.height as usize).rev(){
                for pixel in pixels[row*g.width as usize*4..(row+1)*g.width as usize*4].chunks_exact(4){file.write_all(&pixel[..3])?;}
            }
            file.flush()
        })();
        match self.points[index] {
            CapturePoint::Swap(_)=>rudroid_common::diagnostic!("[SDL] guest frame capture: swap={} size={}x{} result={result:?}",at,g.width,g.height),
            CapturePoint::ElapsedMs(ms)=>rudroid_common::diagnostic!("[SDL] guest frame capture: swap={} after_ms={} elapsed_ms={} size={}x{} result={result:?}",at,ms,self.started.unwrap().elapsed().as_millis(),g.width,g.height),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/frame_capture.rs"]
mod tests;
