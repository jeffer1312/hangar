//! A troca conserva a versão deslocada; conflito não tenta desfazer sobre uma terceira escrita.
use super::Error;
use std::{fs, io, path::Path};

pub(super) fn replace(source: &Path, target: &Path, expected: Option<&[u8]>, link: Option<&Path>) -> Result<(), Error> {
    if expected.is_none() && link.is_none() {
        return move_new(source,target).map_err(|error| if error.kind()==io::ErrorKind::AlreadyExists {
            Error::Changed(format!("Arquivo criado durante a integração: {}",target.display()))
        } else {error.into()});
    }
    let recovery = tempfile::Builder::new().prefix(".hangar-recovery-").tempfile_in(target.parent().ok_or_else(||io::Error::other("Destino sem diretório"))?)?.into_temp_path();
    let recovery = recovery.keep().map_err(|e|e.error)?;
    #[cfg(windows)]
    if expected.is_none() && link.is_some() {
        // ReplaceFileW segue o destino do link; movê-lo conserva o link quebrado.
        fs::rename(target, &recovery)?;
        if fs::read_link(&recovery).ok().as_deref() != link || fs::read(&recovery).is_ok() {
            return Err(Error::Displaced(format!("Link alterado; versão deslocada preservada em {}", recovery.display())));
        }
        if let Err(error) = move_new(source, target) {
            return Err(Error::Displaced(format!("Publicação interrompida; link preservado em {}: {error}", recovery.display())));
        }
        fs::remove_file(&recovery)?;
        return Ok(());
    }
    if let Err(error)=exchange(source,target,&recovery) {
        return Err(Error::Displaced(format!("Falha na troca do arquivo; eventual versão deslocada preservada em {}: {error}",recovery.display())));
    }
    // O arquivo fica retido também quando a leitura falha: nunca apagar uma versão não conferida.
    let displaced=fs::read(&recovery);
    let displaced_link=fs::read_link(&recovery).ok();
    if displaced.as_deref().ok()!=expected || displaced_link.as_deref()!=link {
        return Err(Error::Displaced(format!("Arquivo alterado durante a integração; versão deslocada preservada em {}",recovery.display())));
    }
    fs::remove_file(&recovery)?;
    Ok(())
}
#[cfg(any(target_os="linux",target_os="macos"))]
fn exchange(source:&Path,target:&Path,recovery:&Path)->io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    fs::rename(source,recovery)?;
    let from=std::ffi::CString::new(recovery.as_os_str().as_bytes())?;let to=std::ffi::CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os="linux")]
    let rc=unsafe{libc::renameat2(libc::AT_FDCWD,from.as_ptr(),libc::AT_FDCWD,to.as_ptr(),libc::RENAME_EXCHANGE)};
    #[cfg(target_os="macos")]
    let rc=unsafe{libc::renameatx_np(libc::AT_FDCWD,from.as_ptr(),libc::AT_FDCWD,to.as_ptr(),libc::RENAME_SWAP)};
    if rc!=0{return Err(io::Error::last_os_error())}
    Ok(())
}
#[cfg(windows)]
fn wide(path:&Path)->Vec<u16>{use std::os::windows::ffi::OsStrExt;path.as_os_str().encode_wide().chain(Some(0)).collect()}
#[cfg(windows)]
fn exchange(source:&Path,target:&Path,recovery:&Path)->io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let source=wide(source);let target=wide(target);let recovery=wide(recovery);
    if unsafe{ReplaceFileW(target.as_ptr(),source.as_ptr(),recovery.as_ptr(),0,std::ptr::null(),std::ptr::null())}==0 {Err(io::Error::last_os_error())}else{Ok(())}
}
#[cfg(any(target_os="linux",target_os="macos"))]
fn move_new(source:&Path,target:&Path)->io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from=std::ffi::CString::new(source.as_os_str().as_bytes())?;let to=std::ffi::CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os="linux")]
    let rc=unsafe{libc::renameat2(libc::AT_FDCWD,from.as_ptr(),libc::AT_FDCWD,to.as_ptr(),libc::RENAME_NOREPLACE)};
    #[cfg(target_os="macos")]
    let rc=unsafe{libc::renameatx_np(libc::AT_FDCWD,from.as_ptr(),libc::AT_FDCWD,to.as_ptr(),libc::RENAME_EXCL)};
    if rc==0{Ok(())}else{Err(io::Error::last_os_error())}
}
#[cfg(windows)]
fn move_new(source:&Path,target:&Path)->io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let source=wide(source);let target=wide(target);
    if unsafe{MoveFileExW(source.as_ptr(),target.as_ptr(),0)}==0{Err(io::Error::last_os_error())}else{Ok(())}
}
#[cfg(not(any(target_os="linux",target_os="macos",windows)))]
fn move_new(_: &Path,_:&Path)->io::Result<()>{Err(io::ErrorKind::Unsupported.into())}
#[cfg(not(any(target_os="linux",target_os="macos",windows)))]
fn exchange(_: &Path,_:&Path,_:&Path)->io::Result<()>{Err(io::ErrorKind::Unsupported.into())}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_retains_the_version_written_just_before_exchange() {
        let dir=tempfile::tempdir().unwrap();let target=dir.path().join("config");let source=dir.path().join("new");
        fs::write(&target,b"expected").unwrap();fs::write(&source,b"generated").unwrap();
        let expected=fs::read(&target).unwrap();
        fs::write(&target,b"external edit").unwrap();
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&target,fs::Permissions::from_mode(0o640)).unwrap();}
        let error=replace(&source,&target,Some(&expected),None).unwrap_err();
        assert!(matches!(&error,Error::Displaced(_)));
        let saved=fs::read_dir(dir.path()).unwrap().map(|e|e.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with(".hangar-recovery-")).unwrap();
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;assert_eq!(fs::metadata(&saved).unwrap().permissions().mode()&0o777,0o640);}
        assert_eq!(fs::read(&saved).unwrap(),b"external edit");assert!(error.to_string().contains(saved.to_str().unwrap()));
        fs::write(&target,b"third writer").unwrap();assert_eq!(fs::read(&target).unwrap(),b"third writer");assert_eq!(fs::read(saved).unwrap(),b"external edit");
    }
    #[test]
    fn publication_never_replaces_a_concurrently_created_destination() {
        let dir=tempfile::tempdir().unwrap();let target=dir.path().join("config");let source=dir.path().join("new");
        fs::write(&source,b"generated").unwrap();fs::write(&target,b"external").unwrap();
        assert!(replace(&source,&target,None,None).is_err());assert_eq!(fs::read(target).unwrap(),b"external");
    }
}
