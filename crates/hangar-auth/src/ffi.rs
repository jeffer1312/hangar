//! ABI C: nenhum buffer Rust passa a pertencer ao chamador.
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use crate::{AuthAttempt, AuthConfig, AuthDecision, AuthError, ReceiptContext, SharedLimiter, AUTH_ABI};

fn guarded(op: impl FnOnce() -> Result<(), AuthError>) -> u32 {
    match catch_unwind(AssertUnwindSafe(op)) { Ok(Ok(())) => 0, Ok(Err(e)) => e as u32, Err(_) => AuthError::Panic as u32 }
}
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], AuthError> {
    if len > (1 << 20) || (ptr.is_null() && len != 0) { return Err(AuthError::InvalidInput); }
    if len == 0 { return Ok(&[]); }
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}
unsafe fn handle<'a>(ptr: *mut c_void) -> Result<&'a SharedLimiter, AuthError> {
    if ptr.is_null() { return Err(AuthError::InvalidInput); }
    Ok(unsafe { &*ptr.cast::<SharedLimiter>() })
}
#[unsafe(no_mangle)]
pub extern "C" fn hangar_auth_abi_v1() -> u32 { AUTH_ABI }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hangar_auth_open_v1(data: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    guarded(|| {
        if out.is_null() { return Err(AuthError::InvalidInput); }
        unsafe { *out = std::ptr::null_mut(); }
        let config: AuthConfig = serde_json::from_slice(unsafe { input(data, len)? }).map_err(|_| AuthError::InvalidInput)?;
        let context = Box::new(SharedLimiter::open(config)?);
        unsafe { *out = Box::into_raw(context).cast(); }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hangar_auth_evaluate_v1(ptr: *mut c_void, data: *const u8, len: usize, out: *mut AuthDecision) -> u32 {
    guarded(|| {
        if out.is_null() { return Err(AuthError::InvalidInput); }
        let attempt: AuthAttempt = serde_json::from_slice(unsafe { input(data, len)? }).map_err(|_| AuthError::InvalidInput)?;
        let result = unsafe { handle(ptr)? }.evaluate(&attempt)?;
        unsafe { *out = result; }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hangar_auth_verify_receipt_v1(ptr: *mut c_void, data: *const u8, len: usize,
    receipt: *const u8, receipt_len: usize, out: *mut AuthDecision) -> u32 {
    guarded(|| {
        if out.is_null() { return Err(AuthError::InvalidInput); }
        let context: ReceiptContext = serde_json::from_slice(unsafe { input(data, len)? }).map_err(|_| AuthError::InvalidInput)?;
        let result = unsafe { handle(ptr)? }.verify_receipt(&context, unsafe { input(receipt, receipt_len)? })?;
        unsafe { *out = result; }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hangar_auth_close_v1(ptr: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| { if !ptr.is_null() { drop(unsafe { Box::from_raw(ptr.cast::<SharedLimiter>()) }); } }));
}
