//! Arquivos publicados não se confundem com o corpo ainda em recebimento.
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use unicode_normalization::UnicodeNormalization;

pub const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;
const TEMP_PREFIX: &str = ".upload-";

#[derive(Debug, Serialize)]
pub struct UploadEntry {
    pub filename: String,
    pub size: u64,
    pub mtime: f64,
    pub expires_in_days: Option<f64>,
}

pub fn slug(value: &str) -> String {
    let ascii: String = value.nfkd().filter(char::is_ascii).collect();
    let text: String = ascii
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let text = text.trim_matches('-');
    if text.is_empty() || text == "." || text == ".." {
        "_".into()
    } else {
        text.chars().take(64).collect()
    }
}

/// O hash segue os bytes do caminho real, preservando a caixa do filesystem.
pub fn project_key(cwd: &Path) -> io::Result<String> {
    let real = real_path(cwd)?;
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        real.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let text = real.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    #[cfg(not(unix))]
    let bytes = text.as_bytes();
    let hash = digest::digest(&digest::SHA256, bytes);
    let name = real.file_name().unwrap_or_default().to_string_lossy();
    Ok(format!(
        "{}-{:02x}{:02x}{:02x}",
        slug(&name),
        hash.as_ref()[0],
        hash.as_ref()[1],
        hash.as_ref()[2]
    ))
}

fn real_path(path: &Path) -> io::Result<PathBuf> {
    enum Part {
        Prefix(std::ffi::OsString),
        Root,
        Parent,
        Name(std::ffi::OsString),
        Finished(PathBuf),
    }
    fn parts(path: &Path) -> Vec<Part> {
        path.components()
            .filter_map(|part| match part {
                Component::Prefix(prefix) => Some(Part::Prefix(prefix.as_os_str().to_owned())),
                Component::RootDir => Some(Part::Root),
                Component::CurDir => None,
                Component::ParentDir => Some(Part::Parent),
                Component::Normal(name) => Some(Part::Name(name.to_owned())),
            })
            .collect()
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut pending = parts(&plain_path(absolute)?);
    pending.reverse();
    let mut active = std::collections::HashSet::new();
    let mut resolved = PathBuf::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Prefix(prefix) => resolved = PathBuf::from(prefix),
            Part::Root => resolved.push(std::path::MAIN_SEPARATOR_STR),
            Part::Parent => {
                resolved.pop();
            }
            Part::Finished(link) => {
                active.remove(&link);
            }
            Part::Name(name) => {
                let candidate = resolved.join(name);
                if let Ok(target) = fs::read_link(&candidate) {
                    if active.insert(candidate.clone()) {
                        pending.push(Part::Finished(candidate));
                        pending.extend(parts(&plain_path(target)?).into_iter().rev());
                    } else {
                        // O realpath não estrito conserva o componente de um ciclo.
                        resolved = candidate;
                    }
                } else {
                    resolved = match fs::canonicalize(&candidate) {
                        Ok(real) => plain_path(real)?,
                        Err(_) => candidate,
                    };
                }
            }
        }
    }
    Ok(resolved)
}

fn plain_path(path: PathBuf) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let text = path.to_str().ok_or(io::ErrorKind::InvalidInput)?;
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(path)
}

fn absolute_lexical(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for part in plain_path(absolute)?.components() {
        if part == Component::ParentDir {
            result.pop();
        } else if part != Component::CurDir {
            result.push(part.as_os_str());
        }
    }
    Ok(result)
}

fn component(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    #[cfg(windows)]
    if name.contains(':') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

fn upload_filename(name: &str) -> io::Result<()> {
    component(name)?;
    if name.contains("..") || name.starts_with(TEMP_PREFIX) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

fn extension(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    // O cabeçalho não é URL: percentuais ficam na extensão, como no leitor Python.
    let suffix = name
        .rfind('.')
        .filter(|&p| name[..p].chars().any(|c| c != '.'))
        .map_or("", |p| &name[p + 1..]);
    let ext: String = suffix
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    if ext.is_empty() { "bin".into() } else { ext }
}

fn random_hex(bytes: usize) -> io::Result<String> {
    let mut data = vec![0; bytes];
    SystemRandom::new()
        .fill(&mut data)
        .map_err(|_| io::Error::other("aleatoriedade indisponível"))?;
    Ok(data.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// No Unix, efeitos são relativos ao descritor; no Windows, o handle impede a troca da pasta.
struct Directory {
    path: PathBuf,
    handle: File,
    parent: Option<Arc<Directory>>,
    #[cfg(test)]
    probe: Option<removal_tests::Probe>,
}

impl Directory {
    fn open_root(path: &Path) -> io::Result<Arc<Self>> {
        fs::create_dir_all(path)?;
        let path = real_path(path)?;
        #[cfg(unix)]
        let handle = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)?
        };
        #[cfg(windows)]
        let handle = open_windows_dir(&path)?;
        let directory = Arc::new(Self {
            path,
            handle,
            parent: None,
            #[cfg(test)]
            probe: None,
        });
        directory.verify()?;
        Ok(directory)
    }

    fn child(self: &Arc<Self>, name: &str, create: bool) -> io::Result<Arc<Self>> {
        component(name)?;
        self.verify()?;
        let path = self.path.join(name);
        #[cfg(unix)]
        let handle = {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            if create
                && unsafe { libc::mkdirat(self.handle.as_raw_fd(), name.as_ptr(), 0o777) } != 0
            {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // O descritor recém-aberto passa a ter um único dono.
            unsafe { File::from_raw_fd(fd) }
        };
        #[cfg(windows)]
        let handle = {
            if create {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            // O Windows remove pontos finais; preservar o erro da identidade hostil.
            if real_path(&path)? != path {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            open_windows_dir(&path)?
        };
        let directory = Arc::new(Self {
            path,
            handle,
            parent: Some(self.clone()),
            #[cfg(test)]
            probe: self.probe.clone(),
        });
        directory.verify()?;
        Ok(directory)
    }

    fn verify(&self) -> io::Result<()> {
        if let Some(parent) = &self.parent {
            parent.verify()?;
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        let held = self.handle.metadata()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || !held.is_dir() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.dev() != held.dev() || metadata.ino() != held.ino() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.creation_time() != held.creation_time() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
        if real_path(&self.path)? != self.path {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }

    fn entries(&self) -> io::Result<Vec<String>> {
        self.verify()?;
        let entries = fs::read_dir(&self.path)?
            .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
            .collect();
        self.verify()?;
        Ok(entries)
    }

    fn open_file(&self, name: &str, create: bool) -> io::Result<File> {
        component(name)?;
        self.verify()?;
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            let flags = if create {
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
            } else {
                libc::O_RDONLY
            };
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    0o666,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if !file.metadata()?.is_file() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            Ok(file)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let mut options = OpenOptions::new();
            options
                .read(!create)
                .write(create)
                .create_new(create)
                .custom_flags(0x00200000);
            if create {
                options.access_mode(0x40000000 | 0x00010000).share_mode(3);
            }
            let file = options.open(self.path.join(name))?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            Ok(file)
        }
    }

    fn remove_captured(
        &self,
        name: &str,
        file: &File,
        identity: &fs::Metadata,
        area: &RemovalArea,
        cutoff: Option<f64>,
    ) -> io::Result<bool> {
        component(name)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            let _ = file;
            let captured = format!("captured-{}", random_hex(16)?);
            #[cfg(test)]
            self.reach(removal_tests::Point::BeforeClaim, name);
            rename_exclusive(self, name, &area.directory, &captured)?;
            #[cfg(test)]
            self.reach(removal_tests::Point::AfterClaim, name);
            let captured_file = area.directory.open_file(&captured, false);
            let removable = captured_file
                .as_ref()
                .ok()
                .and_then(|file| file.metadata().ok())
                .is_some_and(|metadata| {
                    metadata.dev() == identity.dev()
                        && metadata.ino() == identity.ino()
                        && cutoff.is_none_or(|limit| {
                            modified_seconds(&metadata).is_ok_and(|mtime| mtime < limit)
                        })
                });
            if !removable {
                #[cfg(test)]
                self.reach(removal_tests::Point::BeforeRestore, name);
                if let Err(error) = rename_exclusive(&area.directory, &captured, self, name) {
                    tracing::warn!(
                        code = "upload_claim_restore_failed",
                        "arquivo capturado preservado sem sobrescrever destino"
                    );
                    return Err(error);
                }
                return Ok(false);
            }
            let native = std::ffi::CString::new(captured)?;
            if unsafe { libc::unlinkat(area.directory.handle.as_raw_fd(), native.as_ptr(), 0) } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(true)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let _ = area;
            let metadata = file.metadata()?;
            if metadata.creation_time() != identity.creation_time()
                || cutoff.is_some_and(|limit| {
                    !modified_seconds(&metadata).is_ok_and(|mtime| mtime < limit)
                })
            {
                return Ok(false);
            }
            mark_file_for_deletion(file)?;
            Ok(true)
        }
    }

    fn open_removable(&self, name: &str) -> io::Result<File> {
        #[cfg(unix)]
        {
            self.open_file(name, false)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            component(name)?;
            self.verify()?;
            let file = OpenOptions::new()
                .access_mode(0x80000000 | 0x00010000)
                .share_mode(3)
                .custom_flags(0x00200000)
                .open(self.path.join(name))?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            Ok(file)
        }
    }

    fn owns_temporary(&self, name: &str, identity: &fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::fs::MetadataExt;
            let Ok(name) = std::ffi::CString::new(name) else {
                return false;
            };
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return false;
            }
            let file = unsafe { File::from_raw_fd(fd) };
            file.metadata().is_ok_and(|m| {
                m.is_file() && m.dev() == identity.dev() && m.ino() == identity.ino()
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            fs::symlink_metadata(self.path.join(name)).is_ok_and(|m| {
                m.is_file()
                    && !m.file_type().is_symlink()
                    && m.creation_time() == identity.creation_time()
            })
        }
    }

    fn publish(
        &self,
        temporary: &str,
        final_name: &str,
        file: &File,
        identity: &fs::Metadata,
        area: &RemovalArea,
    ) -> io::Result<()> {
        self.verify()?;
        if !self.owns_temporary(temporary, identity) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            #[cfg(target_os = "linux")]
            let temporary = std::ffi::CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
            #[cfg(not(target_os = "linux"))]
            let temporary = std::ffi::CString::new(temporary)?;
            let final_name = std::ffi::CString::new(final_name)?;
            #[cfg(target_os = "linux")]
            let flags = libc::AT_SYMLINK_FOLLOW;
            #[cfg(not(target_os = "linux"))]
            let flags = 0;
            let _ = file;
            if unsafe {
                libc::linkat(
                    self.handle.as_raw_fd(),
                    temporary.as_ptr(),
                    self.handle.as_raw_fd(),
                    final_name.as_ptr(),
                    flags,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            if !self.owns_temporary(final_name.to_str().unwrap(), identity) {
                self.remove_captured(final_name.to_str().unwrap(), file, identity, area, None)?;
                return Err(io::ErrorKind::InvalidInput.into());
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let _ = (file, area);
            fs::hard_link(self.path.join(temporary), self.path.join(final_name))
        }
    }
}

#[cfg(windows)]
fn open_windows_dir(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // Sem FILE_SHARE_DELETE: a identidade não pode ser renomeada durante o fluxo.
    let handle = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .custom_flags(0x02000000 | 0x00200000)
        .open(path)?;
    if handle.metadata()?.file_type().is_symlink() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(handle)
}

struct RemovalArea {
    #[cfg(unix)]
    directory: Arc<Directory>,
}

impl RemovalArea {
    fn new(source: &Arc<Directory>) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut root = source.clone();
            while let Some(parent) = &root.parent {
                root = parent.clone();
            }
            root.verify()?;
            let name = format!("{TEMP_PREFIX}{}.claim", random_hex(16)?);
            let native = std::ffi::CString::new(name.as_str())?;
            // A área nasce exclusiva e inacessível a outros usuários.
            if unsafe { libc::mkdirat(root.handle.as_raw_fd(), native.as_ptr(), 0o700) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                directory: root.child(&name, false)?,
            })
        }
        #[cfg(windows)]
        {
            let _ = source;
            Ok(Self {})
        }
    }
}

#[cfg(unix)]
impl Drop for RemovalArea {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let parent = self.directory.parent.as_ref().unwrap();
        let name =
            std::ffi::CString::new(self.directory.path.file_name().unwrap().as_encoded_bytes())
                .unwrap();
        if unsafe { libc::unlinkat(parent.handle.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
            != 0
        {
            // Área não vazia guarda a identidade divergente cuja devolução foi impedida.
            tracing::warn!(
                code = "upload_claim_preserved",
                "área de captura preservada para não apagar arquivo alheio"
            );
        }
    }
}

#[cfg(unix)]
fn rename_exclusive(
    source: &Directory,
    name: &str,
    target: &Directory,
    destination: &str,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let name = std::ffi::CString::new(name)?;
    let destination = std::ffi::CString::new(destination)?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            source.handle.as_raw_fd(),
            name.as_ptr(),
            target.handle.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            source.handle.as_raw_fd(),
            name.as_ptr(),
            target.handle.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err(io::ErrorKind::Unsupported.into());
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn mark_file_for_deletion(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            class: i32,
            information: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let delete: u8 = 1;
    // FileDispositionInfo atua no handle ainda preso à identidade original.
    if unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), 4, (&delete as *const u8).cast(), 1)
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct PartialUpload {
    directory: Arc<Directory>,
    name: String,
    file: Option<File>,
    identity: fs::Metadata,
    removal: RemovalArea,
}

impl Drop for PartialUpload {
    fn drop(&mut self) {
        if let Some(file) = &self.file
            && let Err(error) = self.directory.remove_captured(
                &self.name,
                file,
                &self.identity,
                &self.removal,
                None,
            )
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                code = "upload_temporary_cleanup_failed",
                "não foi possível limpar o temporário do upload"
            );
        }
        // A disposição Windows é decidida antes de soltar o handle.
        self.file.take();
    }
}

#[derive(Clone)]
pub struct UploadStore {
    root: Arc<Directory>,
    trusted_root: PathBuf,
}

impl UploadStore {
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            trusted_root: absolute_lexical(root)?,
            root: Directory::open_root(root)?,
        })
    }

    fn session(&self, project: &str, session: &str, create: bool) -> io::Result<Arc<Directory>> {
        self.root
            .child(project, create)?
            .child(&slug(session), create)
    }

    /// O stream é consumido uma vez; só o temporário criado aqui recebe bytes.
    pub async fn publish<S>(
        &self,
        project: &str,
        session: &str,
        filename: &str,
        length: Option<u64>,
        stream: S,
    ) -> io::Result<PathBuf>
    where
        S: Stream<Item = io::Result<Bytes>>,
    {
        self.publish_named(project, session, filename, length, stream, None).await
    }

    pub async fn publish_derivative<S>(&self, project: &str, session: &str, filename: &str, stream: S) -> io::Result<PathBuf>
    where S: Stream<Item = io::Result<Bytes>> {
        upload_filename(filename)?;
        self.publish_named(project, session, filename, None, stream, Some(filename)).await
    }

    async fn publish_named<S>(&self, project: &str, session: &str, filename: &str, length: Option<u64>, stream: S, named: Option<&str>) -> io::Result<PathBuf>
    where S: Stream<Item = io::Result<Bytes>> {
        if length.is_some_and(|n| n > MAX_UPLOAD_BYTES) {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let directory = self.session(project, session, true)?;
        let removal = RemovalArea::new(&directory)?;
        let name = format!("{TEMP_PREFIX}{}.tmp", random_hex(16)?);
        let file = directory.open_file(&name, true)?;
        let identity = file.metadata()?;
        let mut partial = PartialUpload {
            directory,
            name,
            file: Some(file),
            identity,
            removal,
        };
        let mut size = 0u64;
        futures_util::pin_mut!(stream);
        while let Some(block) = stream.next().await {
            let block = block?;
            size = size
                .checked_add(block.len() as u64)
                .ok_or(io::ErrorKind::FileTooLarge)?;
            if size > MAX_UPLOAD_BYTES {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            // Blocos pequenos limitam a escrita e dão oportunidade ao cancelamento.
            for chunk in block.chunks(64 * 1024) {
                partial.file.as_mut().unwrap().write_all(chunk)?;
                tokio::task::yield_now().await;
            }
        }
        if size == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        partial.file.as_ref().unwrap().sync_all()?;
        let ext = extension(filename);
        let timestamp = chrono::Utc::now().timestamp();
        for _ in 0..128 {
            let final_name = match named {
                Some(name) => name.to_owned(),
                None => format!("{timestamp}-{}.{ext}", random_hex(3)?),
            };
            match partial.directory.publish(
                &partial.name,
                &final_name,
                partial.file.as_ref().unwrap(),
                &partial.identity,
                &partial.removal,
            ) {
                Ok(()) => return Ok(partial.directory.path.join(final_name)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && named.is_none() => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "não foi possível reservar um nome de upload",
        ))
    }

    pub fn list(
        &self,
        project: &str,
        session: &str,
        days: i64,
        now: f64,
    ) -> io::Result<Vec<UploadEntry>> {
        let directory = match self.session(project, session, false) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let mut entries = Vec::new();
        for name in directory.entries()? {
            if name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let Ok(file) = directory.open_file(&name, false) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let mtime = match modified.duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_secs_f64(),
                Err(e) => -e.duration().as_secs_f64(),
            };
            entries.push(UploadEntry {
                filename: name,
                size: metadata.len(),
                mtime,
                expires_in_days: (days > 0).then_some(days as f64 - (now - mtime) / 86400.0),
            });
        }
        entries.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        Ok(entries)
    }

    /// Valida o caminho legado. Quem precisar reter identidade usa open, não reabre o path.
    pub fn resolve(&self, project: &str, session: &str, filename: &str) -> io::Result<PathBuf> {
        let (path, _) = self.open(project, session, filename)?;
        Ok(path)
    }

    pub fn open(
        &self,
        project: &str,
        session: &str,
        filename: &str,
    ) -> io::Result<(PathBuf, File)> {
        upload_filename(filename)?;
        let directory = self.session(project, session, false)?;
        let path = directory.path.join(filename);
        let file = directory.open_file(filename, false)?;
        if real_path(&path)? != path {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok((path, file))
    }

    pub fn resolve_audio(
        &self,
        project: &str,
        session: &str,
        reference: &str,
        allow_absolute: bool,
    ) -> io::Result<PathBuf> {
        if reference.contains('\0') || reference.starts_with("//") || reference.starts_with(r"\\") {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let path = Path::new(reference);
        if !path.is_absolute() {
            return self.resolve(project, session, reference);
        }
        if !allow_absolute {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        component(project)?;
        let base = self.root.path.join(project);
        // Checar contenção lexical evita até consultar referências de outro projeto.
        contained_path(&base, path)
            .or_else(|| contained_path(&self.trusted_root.join(project), path))
            .ok_or(io::ErrorKind::InvalidInput)?;
        let real = real_path(path)?;
        let relative = contained_path(&base, &real).ok_or(io::ErrorKind::InvalidInput)?;
        let parts: Vec<_> = relative.components().collect();
        let [Component::Normal(session_name), Component::Normal(filename)] = parts.as_slice()
        else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let session_name = session_name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
        let filename = filename.to_str().ok_or(io::ErrorKind::InvalidInput)?;
        self.resolve(project, session_name, filename)
    }

    pub fn prune(&self, project: &str, days: i64, now: f64) -> io::Result<usize> {
        if days <= 0 {
            return Ok(0);
        }
        let directory = match self.root.child(project, false) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        prune_directory(&directory, now - days as f64 * 86400.0)
    }
}

fn contained_path(base: &Path, path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut actual = path.components();
        for expected in base.components() {
            let component = actual.next()?;
            if component.as_os_str().to_string_lossy().to_lowercase()
                != expected.as_os_str().to_string_lossy().to_lowercase()
            {
                return None;
            }
        }
        Some(actual.as_path().to_path_buf())
    }
    #[cfg(not(windows))]
    {
        path.strip_prefix(base).ok().map(Path::to_path_buf)
    }
}

fn prune_directory(directory: &Arc<Directory>, cutoff: f64) -> io::Result<usize> {
    let mut removed = 0;
    let area = RemovalArea::new(directory)?;
    for name in directory.entries()? {
        if name.starts_with(TEMP_PREFIX) {
            continue;
        }
        if let Ok(child) = directory.child(&name, false) {
            removed += prune_directory(&child, cutoff).unwrap_or(0);
            continue;
        }
        let Ok(file) = directory.open_removable(&name) else {
            continue;
        };
        let Ok(identity) = file.metadata() else {
            continue;
        };
        if modified_seconds(&identity).is_ok_and(|mtime| mtime < cutoff)
            && directory.verify().is_ok()
            && directory
                .remove_captured(&name, &file, &identity, &area, Some(cutoff))
                .unwrap_or(false)
        {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::{Mutex, mpsc};
    #[cfg(unix)]
    use std::time::Duration;

    /// Pontos da remoção real em que um teste pode intercalar outro ator.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) enum Point {
        BeforeClaim,
        AfterClaim,
        BeforeRestore,
    }

    /// Sincronização local ao `Directory` (e aos filhos dele): testes paralelos não se misturam.
    pub(super) type Probe = Arc<dyn Fn(Point, &str) + Send + Sync>;

    #[cfg(unix)]
    impl Directory {
        pub(super) fn reach(&self, point: Point, name: &str) {
            if let Some(probe) = &self.probe {
                probe(point, name);
            }
        }
    }

    #[cfg(unix)]
    impl UploadStore {
        fn with_probe(root: &Path, probe: Probe) -> io::Result<Self> {
            let mut store = Self::new(root)?;
            Arc::get_mut(&mut store.root)
                .expect("raiz recém-aberta tem um único dono")
                .probe = Some(probe);
            Ok(store)
        }
    }

    #[cfg(unix)]
    const HANDSHAKE: Duration = Duration::from_secs(20);

    /// O fluxo real para no ponto, avisa o teste e só segue quando o teste devolver a vez.
    #[cfg(unix)]
    struct Rendezvous {
        reached: mpsc::Receiver<(Point, String)>,
        resume: mpsc::SyncSender<()>,
    }

    #[cfg(unix)]
    impl Rendezvous {
        fn at(&self, expected: Point) -> String {
            let (point, name) = self
                .reached
                .recv_timeout(HANDSHAKE)
                .expect("o fluxo de remoção não chegou ao ponto no prazo");
            assert_eq!(point, expected);
            name
        }

        fn release(&self) {
            self.resume
                .send(())
                .expect("o fluxo de remoção desistiu do handshake");
        }
    }

    #[cfg(unix)]
    fn rendezvous(points: &'static [Point]) -> (Probe, Rendezvous) {
        let (reached_tx, reached) = mpsc::channel();
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let resume_rx = Mutex::new(resume_rx);
        let probe: Probe = Arc::new(move |point, name: &str| {
            if !points.contains(&point) {
                return;
            }
            reached_tx
                .send((point, name.to_owned()))
                .expect("o teste saiu antes do ponto");
            resume_rx
                .lock()
                .unwrap()
                .recv_timeout(HANDSHAKE)
                .expect("o teste não devolveu a vez no prazo");
        });
        (probe, Rendezvous { reached, resume })
    }

    #[cfg(unix)]
    fn probed_root(path: &Path, probe: Probe) -> Arc<Directory> {
        let mut directory = Directory::open_root(path).unwrap();
        Arc::get_mut(&mut directory).unwrap().probe = Some(probe);
        directory
    }

    #[cfg(unix)]
    fn set_modified(path: &Path, seconds: u64) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }

    /// Arquivos dentro das áreas de captura (`.upload-*.claim`) da raiz.
    #[cfg(unix)]
    fn captured_files(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with(TEMP_PREFIX) && name.ends_with(".claim") {
                for inner in fs::read_dir(&path).unwrap() {
                    found.push(inner.unwrap().path());
                }
            }
        }
        found
    }

    #[cfg(unix)]
    fn names(path: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_swap_before_claim_keeps_substitute_and_moved_original() {
        use std::os::unix::fs::MetadataExt;
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::BeforeClaim]);
        let directory = probed_root(fixture.path(), probe);
        let area = RemovalArea::new(&directory).unwrap();
        let mut original = directory.open_file("target.bin", true).unwrap();
        original.write_all(b"original").unwrap();
        let identity = original.metadata().unwrap();
        let worker = {
            let directory = directory.clone();
            std::thread::spawn(move || {
                let result =
                    directory.remove_captured("target.bin", &original, &identity, &area, None);
                (result.map_err(|e| e.kind()), area)
            })
        };
        assert_eq!(meet.at(Point::BeforeClaim), "target.bin");
        fs::rename(
            fixture.path().join("target.bin"),
            fixture.path().join("moved.bin"),
        )
        .unwrap();
        fs::write(fixture.path().join("target.bin"), b"substituto").unwrap();
        let foreign = fs::metadata(fixture.path().join("target.bin"))
            .unwrap()
            .ino();
        meet.release();
        let (result, area) = worker.join().unwrap();
        assert_eq!(result, Ok(false));
        assert_eq!(
            fs::metadata(fixture.path().join("target.bin"))
                .unwrap()
                .ino(),
            foreign
        );
        assert_eq!(
            fs::read(fixture.path().join("target.bin")).unwrap(),
            b"substituto"
        );
        assert_eq!(
            fs::read(fixture.path().join("moved.bin")).unwrap(),
            b"original"
        );
        drop(area);
        assert_eq!(names(fixture.path()), ["moved.bin", "target.bin"]);
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_occupant_blocks_prune_rollback_without_overwrite() {
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::AfterClaim, Point::BeforeRestore]);
        let directory = probed_root(fixture.path(), probe);
        fs::write(fixture.path().join("old.bin"), b"original").unwrap();
        set_modified(&fixture.path().join("old.bin"), 1);
        let worker =
            std::thread::spawn(move || prune_directory(&directory, 86400.0).map_err(|e| e.kind()));
        assert_eq!(meet.at(Point::AfterClaim), "old.bin");
        let captured = captured_files(fixture.path());
        assert_eq!(captured.len(), 1);
        // Recente entre a captura e a decisão: a poda tem de devolver o arquivo.
        set_modified(&captured[0], 172800);
        meet.release();
        assert_eq!(meet.at(Point::BeforeRestore), "old.bin");
        fs::write(fixture.path().join("old.bin"), b"ocupante").unwrap();
        meet.release();
        assert_eq!(worker.join().unwrap(), Ok(0));
        assert_eq!(
            fs::read(fixture.path().join("old.bin")).unwrap(),
            b"ocupante"
        );
        assert_eq!(captured_files(fixture.path()), captured);
        assert_eq!(fs::read(&captured[0]).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_upload_removes_only_its_temporary_during_concurrent_publication() {
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::AfterClaim]);
        let store = UploadStore::with_probe(fixture.path(), probe).unwrap();
        let (written_tx, written_rx) = tokio::sync::oneshot::channel::<()>();
        let stream = futures_util::stream::once(async { Ok(Bytes::from_static(b"cancelado")) })
            .chain(futures_util::stream::once(async move {
                let _ = written_tx.send(());
                futures_util::future::pending::<io::Result<Bytes>>().await
            }));
        let mut upload = Box::pin(store.publish("project", "session", "a.bin", None, stream));
        tokio::select! {
            _ = &mut upload => panic!("o upload sem fim terminou"),
            _ = written_rx => {}
        }
        let root = fixture.path().to_path_buf();
        let actor = std::thread::spawn(move || {
            let name = meet.at(Point::AfterClaim);
            assert!(
                name.starts_with(TEMP_PREFIX) && name.ends_with(".tmp"),
                "{name}"
            );
            let other = UploadStore::new(&root).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let published = runtime
                .block_on(other.publish(
                    "project",
                    "session",
                    "b.bin",
                    None,
                    futures_util::stream::iter([Ok(Bytes::from_static(b"concorrente"))]),
                ))
                .unwrap();
            meet.release();
            published
        });
        drop(upload);
        let published = actor.join().unwrap();
        let session = fixture.path().join("project/session");
        assert_eq!(
            names(&session),
            [published
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        assert_eq!(fs::read(&published).unwrap(), b"concorrente");
        assert_eq!(names(fixture.path()), ["project"]);
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_prune_swap_before_claim_keeps_new_file() {
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::BeforeClaim]);
        let directory = probed_root(fixture.path(), probe);
        fs::write(fixture.path().join("old.bin"), b"original").unwrap();
        set_modified(&fixture.path().join("old.bin"), 1);
        let worker =
            std::thread::spawn(move || prune_directory(&directory, 86400.0).map_err(|e| e.kind()));
        assert_eq!(meet.at(Point::BeforeClaim), "old.bin");
        fs::rename(
            fixture.path().join("old.bin"),
            fixture.path().join("aside.bin"),
        )
        .unwrap();
        fs::write(fixture.path().join("old.bin"), b"novo").unwrap();
        // Também vencido: só a identidade capturada pode impedir a remoção.
        set_modified(&fixture.path().join("old.bin"), 1);
        meet.release();
        assert_eq!(worker.join().unwrap(), Ok(0));
        assert_eq!(fs::read(fixture.path().join("old.bin")).unwrap(), b"novo");
        assert_eq!(
            fs::read(fixture.path().join("aside.bin")).unwrap(),
            b"original"
        );
        assert_eq!(names(fixture.path()), ["aside.bin", "old.bin"]);
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_mtime_at_strict_cutoff_after_claim_keeps_file() {
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::AfterClaim]);
        let directory = probed_root(fixture.path(), probe);
        fs::write(fixture.path().join("old.bin"), b"original").unwrap();
        set_modified(&fixture.path().join("old.bin"), 1);
        let worker =
            std::thread::spawn(move || prune_directory(&directory, 86400.0).map_err(|e| e.kind()));
        assert_eq!(meet.at(Point::AfterClaim), "old.bin");
        let captured = captured_files(fixture.path());
        assert_eq!(captured.len(), 1);
        // Exatamente no corte: o limite é estrito, então o arquivo deixa de ser vencido.
        set_modified(&captured[0], 86400);
        meet.release();
        assert_eq!(worker.join().unwrap(), Ok(0));
        assert_eq!(
            fs::read(fixture.path().join("old.bin")).unwrap(),
            b"original"
        );
        assert_eq!(
            modified_seconds(&fs::metadata(fixture.path().join("old.bin")).unwrap()).unwrap(),
            86400.0
        );
        assert_eq!(names(fixture.path()), ["old.bin"]);
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_rollback_failure_is_identifiable_and_preserves_both_identities() {
        use std::os::unix::fs::MetadataExt;
        let fixture = tempfile::tempdir().unwrap();
        let (probe, meet) = rendezvous(&[Point::BeforeClaim, Point::BeforeRestore]);
        let directory = probed_root(fixture.path(), probe);
        let area = RemovalArea::new(&directory).unwrap();
        let area_path = area.directory.path.clone();
        let mut original = directory.open_file("target.bin", true).unwrap();
        original.write_all(b"original").unwrap();
        let identity = original.metadata().unwrap();
        let worker = {
            let directory = directory.clone();
            std::thread::spawn(move || {
                let result =
                    directory.remove_captured("target.bin", &original, &identity, &area, None);
                (result.map_err(|e| e.kind()), area)
            })
        };
        assert_eq!(meet.at(Point::BeforeClaim), "target.bin");
        fs::rename(
            fixture.path().join("target.bin"),
            fixture.path().join("moved.bin"),
        )
        .unwrap();
        fs::write(fixture.path().join("target.bin"), b"substituto").unwrap();
        let foreign = fs::metadata(fixture.path().join("target.bin"))
            .unwrap()
            .ino();
        meet.release();
        assert_eq!(meet.at(Point::BeforeRestore), "target.bin");
        fs::write(fixture.path().join("target.bin"), b"ocupante").unwrap();
        meet.release();
        let (result, area) = worker.join().unwrap();
        assert_eq!(result, Err(io::ErrorKind::AlreadyExists));
        assert_eq!(
            fs::read(fixture.path().join("target.bin")).unwrap(),
            b"ocupante"
        );
        assert_eq!(
            fs::read(fixture.path().join("moved.bin")).unwrap(),
            b"original"
        );
        let captured = captured_files(fixture.path());
        assert_eq!(captured.len(), 1);
        assert_eq!(fs::metadata(&captured[0]).unwrap().ino(), foreign);
        assert_eq!(fs::read(&captured[0]).unwrap(), b"substituto");
        drop(area);
        assert!(
            area_path.is_dir(),
            "área com a identidade capturada foi apagada"
        );
    }

    #[test]
    fn native_removal_keeps_published_link_and_previous_file() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = Directory::open_root(fixture.path()).unwrap();
        let area = RemovalArea::new(&directory).unwrap();
        fs::write(directory.path.join("previous.bin"), b"anterior").unwrap();
        let mut file = directory.open_file(".upload-original.tmp", true).unwrap();
        file.write_all(b"original").unwrap();
        let identity = file.metadata().unwrap();
        fs::hard_link(
            directory.path.join(".upload-original.tmp"),
            directory.path.join("published.bin"),
        )
        .unwrap();
        assert!(
            directory
                .remove_captured(".upload-original.tmp", &file, &identity, &area, None)
                .unwrap()
        );
        drop(file);
        assert!(!directory.path.join(".upload-original.tmp").exists());
        assert_eq!(
            fs::read(directory.path.join("published.bin")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(directory.path.join("previous.bin")).unwrap(),
            b"anterior"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_claim_restores_divergent_identity_without_overwrite() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let fixture = tempfile::tempdir().unwrap();
        let directory = Directory::open_root(fixture.path()).unwrap();
        let area = RemovalArea::new(&directory).unwrap();
        let mut original = directory.open_file("target.bin", true).unwrap();
        original.write_all(b"original").unwrap();
        let identity = original.metadata().unwrap();
        fs::rename(
            directory.path.join("target.bin"),
            directory.path.join("moved.bin"),
        )
        .unwrap();
        fs::write(directory.path.join("target.bin"), b"substituto").unwrap();
        let foreign = fs::metadata(directory.path.join("target.bin")).unwrap();
        assert_ne!(foreign.ino(), identity.ino());
        assert!(
            !directory
                .remove_captured("target.bin", &original, &identity, &area, None)
                .unwrap()
        );
        assert_eq!(
            fs::metadata(directory.path.join("target.bin"))
                .unwrap()
                .ino(),
            foreign.ino()
        );
        assert_eq!(
            fs::read(directory.path.join("target.bin")).unwrap(),
            b"substituto"
        );
        assert_eq!(
            fs::read(directory.path.join("moved.bin")).unwrap(),
            b"original"
        );
        fs::write(
            area.directory.path.join("captured-test"),
            b"substituto capturado",
        )
        .unwrap();
        assert_eq!(
            rename_exclusive(&area.directory, "captured-test", &directory, "target.bin")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(area.directory.path.join("captured-test")).unwrap(),
            b"substituto capturado"
        );
        assert_eq!(
            fs::read(directory.path.join("target.bin")).unwrap(),
            b"substituto"
        );
        assert_eq!(
            fs::metadata(&area.directory.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn native_removal_rechecks_modified_time_on_retained_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = Directory::open_root(fixture.path()).unwrap();
        let area = RemovalArea::new(&directory).unwrap();
        fs::write(directory.path.join("old.bin"), b"original").unwrap();
        let writable = OpenOptions::new()
            .write(true)
            .open(directory.path.join("old.bin"))
            .unwrap();
        writable
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let file = directory.open_removable("old.bin").unwrap();
        let identity = file.metadata().unwrap();
        assert!(modified_seconds(&identity).unwrap() < 86400.0);
        writable
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(172800)),
            )
            .unwrap();
        drop(writable);
        assert!(
            !directory
                .remove_captured("old.bin", &file, &identity, &area, Some(86400.0))
                .unwrap()
        );
        assert_eq!(
            fs::read(directory.path.join("old.bin")).unwrap(),
            b"original"
        );
        assert_eq!(
            modified_seconds(&file.metadata().unwrap()).unwrap(),
            172800.0
        );
    }
}

fn modified_seconds(metadata: &fs::Metadata) -> io::Result<f64> {
    Ok(match metadata.modified()?.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    })
}
