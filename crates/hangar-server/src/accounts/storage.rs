//! Arquivos publicados por último e remoção que não atravessa links.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const PENDING: &str = ".hangar-account-pending";
pub const CLAUDE_MARKER: &str = ".hangar-conta";
pub const CODEX_MARKER: &str = ".hangar-codex-conta";

pub fn real_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}
pub fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}
pub fn pending(path: &Path) -> bool {
    fs::symlink_metadata(path.join(PENDING)).is_ok()
}

struct DirectoryIdentity {
    _file: fs::File,
    volume: u64,
    index: u64,
}
impl PartialEq for DirectoryIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.volume == other.volume && self.index == other.index
    }
}
fn identity(path: &Path) -> io::Result<DirectoryIdentity> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02000000 | 0x00200000).share_mode(7);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other("pasta trocada"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(DirectoryIdentity {
            _file: file,
            volume: metadata.dev(),
            index: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct FileInformation {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                information: *mut FileInformation,
            ) -> i32;
        }
        let mut information = FileInformation::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(DirectoryIdentity {
            _file: file,
            volume: information.volume as u64,
            index: ((information.index_high as u64) << 32) | information.index_low as u64,
        })
    }
}

pub struct NewDirectory {
    path: PathBuf,
    identity: DirectoryIdentity,
    published: bool,
}
impl NewDirectory {
    pub fn create(path: &Path) -> io::Result<Self> {
        #[allow(unused_mut)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
        let directory = Self {
            path: path.into(),
            identity: identity(path)?,
            published: false,
        };
        fs::write(path.join(PENDING), b"")?;
        Ok(directory)
    }
    pub fn publish(mut self, marker: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        if identity(&self.path)? != self.identity {
            return Err(io::Error::other("pasta trocada"));
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path.join(marker))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::remove_file(self.path.join(PENDING))?;
        self.published = true;
        Ok(())
    }
}
impl Drop for NewDirectory {
    fn drop(&mut self) {
        if !self.published
            && identity(&self.path).is_ok_and(|id| id == self.identity)
            && let Err(error) = remove_tree(&self.path)
        {
            tracing::warn!(error=?error.kind(), "rollback de cadastro não concluiu");
        }
    }
}

pub fn remove_tree(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if meta.file_type().is_symlink_dir() {
                return fs::remove_dir(path);
            }
        }
        return fs::remove_file(path);
    }
    if meta.is_dir() {
        // Por descritor: uma subpasta trocada por link no meio da remoção não leva o que fica fora.
        fs::remove_dir_all(path)
    } else {
        #[cfg(windows)]
        if meta.permissions().readonly() {
            let mut permissions = meta.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
        }
        fs::remove_file(path)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    #[test]
    fn removal_does_not_follow_a_folder_swapped_for_a_link() {
        // A troca depende de qual thread anda primeiro: repete até ela acontecer no meio da
        // remoção, e em toda tentativa que trocou nada de fora pode sumir.
        for _ in 0..20 {
            if let Some(kept) = swap_during_removal() {
                assert_eq!(kept, 4000, "a remoção apagou arquivos fora da conta");
                return;
            }
        }
        panic!("a troca nunca aconteceu no meio da remoção");
    }

    /// Remove a conta enquanto outra thread troca `projects` por um link para fora, de forma
    /// atômica, assim que a pasta muda. Devolve quantos arquivos de fora sobraram, ou nada se a
    /// remoção terminou antes da troca.
    fn swap_during_removal() -> Option<usize> {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        let inner = account.join("projects");
        let outside = root.path().join("outside");
        let link = root.path().join("link");
        fs::create_dir_all(&inner).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let names: Vec<_> = (0..4000).map(|i| format!("{i}.jsonl")).collect();
        for name in &names {
            fs::write(inner.join(name), b"conta").unwrap();
            fs::write(outside.join(name), b"alheio").unwrap();
        }
        let before = fs::metadata(&inner).unwrap();
        let (from, to) = (
            CString::new(inner.as_os_str().as_bytes()).unwrap(),
            CString::new(link.as_os_str().as_bytes()).unwrap(),
        );
        let swapper = std::thread::spawn({
            let inner = inner.clone();
            move || loop {
                match fs::metadata(&inner) {
                    Ok(now) if (now.mtime(), now.mtime_nsec()) == (before.mtime(), before.mtime_nsec()) => {
                        std::hint::spin_loop()
                    }
                    Ok(_) => unsafe {
                        return libc::renameat2(
                            libc::AT_FDCWD,
                            from.as_ptr(),
                            libc::AT_FDCWD,
                            to.as_ptr(),
                            libc::RENAME_EXCHANGE,
                        ) == 0;
                    },
                    Err(_) => return false,
                }
            }
        });
        let _ = remove_tree(&account);
        if !swapper.join().unwrap() {
            return None;
        }
        Some(names.iter().filter(|name| outside.join(name).exists()).count())
    }
}
