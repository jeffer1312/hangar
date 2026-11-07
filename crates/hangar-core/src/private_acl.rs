//! Descritor de segurança do Windows que dá o objeto só ao usuário corrente.
use std::io;
use std::path::Path;

pub struct SecurityDescriptor(pub windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {unsafe {windows_sys::Win32::Foundation::LocalFree(self.0);}}
}
pub fn owner_security()->io::Result<SecurityDescriptor> {owner_security_with("")}
fn owner_security_with(inherit:&str)->io::Result<SecurityDescriptor> {
    use windows_sys::Win32::{Foundation::{CloseHandle,LocalFree},Security::{TOKEN_QUERY,TOKEN_USER,TokenUser,GetTokenInformation,Authorization::{ConvertSidToStringSidW,ConvertStringSecurityDescriptorToSecurityDescriptorW}},System::Threading::{GetCurrentProcess,OpenProcessToken}};
    let mut token=std::ptr::null_mut();
    // SAFETY: saída local, token só para leitura da identidade corrente.
    if unsafe {OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&mut token)}==0 {return Err(io::Error::last_os_error());}
    let mut size=0;
    unsafe {GetTokenInformation(token,TokenUser,std::ptr::null_mut(),0,&mut size);}
    let mut storage=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
    let result=unsafe {GetTokenInformation(token,TokenUser,storage.as_mut_ptr().cast(),size,&mut size)};
    unsafe {CloseHandle(token);}
    if result==0 {return Err(io::Error::last_os_error());}
    let user=unsafe {&*(storage.as_ptr().cast::<TOKEN_USER>())};
    let mut sid=std::ptr::null_mut();
    if unsafe {ConvertSidToStringSidW(user.User.Sid,&mut sid)}==0 {return Err(io::Error::last_os_error());}
    let mut len=0;while unsafe {*sid.add(len)}!=0 {len+=1;}
    let sid_text=String::from_utf16(unsafe {std::slice::from_raw_parts(sid,len)}).map_err(|_|io::ErrorKind::InvalidData);
    unsafe {LocalFree(sid.cast());}
    let sid_text=sid_text?;
    let sddl:Vec<u16>=format!("O:{sid_text}D:P(A;{inherit};GA;;;{sid_text})\0").encode_utf16().collect();
    let mut descriptor=std::ptr::null_mut();
    // SAFETY: SDDL terminado em NUL; o descritor vive até CreateNamedPipe voltar.
    if unsafe {ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,std::ptr::null_mut())}==0 {return Err(io::Error::last_os_error());}
    Ok(SecurityDescriptor(descriptor))
}
/// `inherit` é o campo de herança da ACE: `"OICI"` faz o que nascer dentro da pasta sair restrito também.
pub fn restrict_path(path:&Path,inherit:&str)->io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{SetFileSecurityW,OWNER_SECURITY_INFORMATION,DACL_SECURITY_INFORMATION,PROTECTED_DACL_SECURITY_INFORMATION};
    let descriptor=owner_security_with(inherit)?;
    let path:Vec<_>=path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {SetFileSecurityW(path.as_ptr(),OWNER_SECURITY_INFORMATION|DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,descriptor.0)}==0 {return Err(io::Error::last_os_error());}
    Ok(())
}
