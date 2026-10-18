//! INPUT:  isolated VM-owned and borrowed direct buffers
//! OUTPUT:  rooted stable zeroing and explicit borrowed-memory rejection
//! POS:    checks the PCM native boundary without original game buffers
use super::*;

#[test]
fn pcm_zeroing_preserves_owned_address_and_rejects_borrowed_storage(){
    let _serial=super::super::test_support::SERIAL.lock().unwrap_or_else(|p|p.into_inner());let s=Session::new();s.install_host(super::super::test_support::stub_host());let buffer=s.allocate_direct_buffer(16);let address=s.direct_buffer_address(buffer).cast::<u8>();
    // SAFETY: isolated rooted owned buffer with no concurrent JNI/native accesses.
    unsafe{std::ptr::write_bytes(address,0x7f,16);}
    s.zero_owned_buffer(buffer);assert!(s.take_pending().is_none());assert_eq!(s.direct_buffer_address(buffer).cast::<u8>(),address);
    // SAFETY: same rooted isolated allocation; zeroing finished synchronously.
    unsafe{assert!(std::slice::from_raw_parts(address,16).iter().all(|b|*b==0));}
    let mut borrowed=[0x55u8;16];let external=s.direct_buffer(Buffer::Borrowed{address:borrowed.as_mut_ptr() as usize,capacity:16});s.zero_owned_buffer(external);assert!(s.take_pending().is_some());assert_eq!(borrowed,[0x55;16]);s.delete_ref(external,false,"test");s.delete_ref(buffer,false,"test");
}
