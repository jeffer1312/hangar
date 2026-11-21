use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::VideoError;
use crate::ffi;

/// Variável com o caminho exato da libmpv; vence os nomes do sistema.
pub const ENV_PATH: &str = "HANGAR_LIBMPV";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Os { Linux, Windows, Macos, Other }

impl Os {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "linux") { Os::Linux }
        else if cfg!(target_os = "windows") { Os::Windows }
        else if cfg!(target_os = "macos") { Os::Macos }
        else { Os::Other }
    }
}

/// Onde procurar, na ordem: o caminho de `HANGAR_LIBMPV`, a pasta do executável (Windows e o `Frameworks` do pacote
/// do macOS, onde um instalador a deixaria), os nomes que o carregador do sistema procura sozinho e, no macOS, as
/// pastas do Homebrew, que um app aberto pelo Finder não enxerga.
pub(crate) fn candidates(os: Os, explicit: Option<OsString>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = explicit.filter(|path| !path.is_empty()) { out.push(PathBuf::from(path)); }
    let names: &[&str] = match os {
        Os::Linux => &["libmpv.so.2", "libmpv.so"],
        Os::Windows => &["libmpv-2.dll", "mpv-2.dll"],
        Os::Macos => &["libmpv.2.dylib", "libmpv.dylib"],
        Os::Other => &[],
    };
    if let Some(dir) = exe_dir {
        match os {
            Os::Windows => out.extend(names.iter().map(|name| dir.join(name))),
            Os::Macos => out.extend(names.iter().map(|name| dir.join("../Frameworks").join(name))),
            Os::Linux | Os::Other => {}
        }
    }
    out.extend(names.iter().map(PathBuf::from));
    if os == Os::Macos {
        for dir in ["/opt/homebrew/lib", "/usr/local/lib"] { out.extend(names.iter().map(|name| Path::new(dir).join(name))); }
    }
    out
}

/// A libmpv carregada e as funções dela. Nunca é descarregada enquanto um player a usa: cada um guarda o `Arc`.
pub struct Library {
    pub(crate) api: ffi::Api,
    path: PathBuf,
    _lib: libloading::Library,
}

/// Só o sucesso fica guardado: instalar o mpv com o app aberto vale na próxima tentativa, sem reabrir.
static SHARED: Mutex<Option<Arc<Library>>> = Mutex::new(None);

impl Library {
    pub fn shared() -> Result<Arc<Library>, VideoError> {
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(lib) = shared.as_ref() { return Ok(lib.clone()); }
        let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
        let lib = Arc::new(Self::load(&candidates(Os::current(), std::env::var_os(ENV_PATH), exe_dir.as_deref()))?);
        *shared = Some(lib.clone());
        Ok(lib)
    }

    /// Caminho ou nome por onde ela foi achada, para o diário.
    pub fn path(&self) -> &Path { &self.path }

    fn load(candidates: &[PathBuf]) -> Result<Library, VideoError> {
        let mut tried = Vec::new();
        for path in candidates {
            // SAFETY: carregar roda os construtores da biblioteca; é a libmpv que o usuário instalou ou apontou.
            let lib = match unsafe { libloading::Library::new(path) } {
                Ok(lib) => lib,
                Err(error) => { tried.push(format!("{}: {error}", path.display())); continue; }
            };
            let api = match resolve(&lib) {
                Ok(api) => api,
                Err(why) => { tried.push(format!("{}: {why}", path.display())); continue; }
            };
            // SAFETY: função sem argumentos, resolvida desta biblioteca.
            let version = unsafe { (api.client_api_version)() };
            if ffi::api_major(version) != 2 {
                tried.push(format!("{}: client API {}.{}, need 2.x", path.display(), version >> 16, version & 0xffff));
                continue;
            }
            return Ok(Library { api, path: path.clone(), _lib: lib });
        }
        Err(VideoError::Unavailable(if tried.is_empty() { "no candidate for this system".into() } else { tried.join("; ") }))
    }
}

/// SAFETY de quem chama: `T` é o tipo de ponteiro de função do protótipo C de `name`.
unsafe fn symbol<T: Copy>(lib: &libloading::Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: repassada a quem chama.
    unsafe { lib.get::<T>(name) }.map(|symbol| *symbol)
        .map_err(|_| format!("missing {}", String::from_utf8_lossy(name.strip_suffix(b"\0").unwrap_or(name))))
}

fn resolve(lib: &libloading::Library) -> Result<ffi::Api, String> {
    // SAFETY: cada tipo é o protótipo de `client.h`/`render.h` (ver `ffi::Api`).
    unsafe {
        Ok(ffi::Api {
            client_api_version: symbol(lib, b"mpv_client_api_version\0")?,
            create: symbol(lib, b"mpv_create\0")?,
            initialize: symbol(lib, b"mpv_initialize\0")?,
            terminate_destroy: symbol(lib, b"mpv_terminate_destroy\0")?,
            set_option_string: symbol(lib, b"mpv_set_option_string\0")?,
            command_async: symbol(lib, b"mpv_command_async\0")?,
            observe_property: symbol(lib, b"mpv_observe_property\0")?,
            wait_event: symbol(lib, b"mpv_wait_event\0")?,
            wakeup: symbol(lib, b"mpv_wakeup\0")?,
            error_string: symbol(lib, b"mpv_error_string\0")?,
            render_context_create: symbol(lib, b"mpv_render_context_create\0")?,
            render_context_set_update_callback: symbol(lib, b"mpv_render_context_set_update_callback\0")?,
            render_context_update: symbol(lib, b"mpv_render_context_update\0")?,
            render_context_render: symbol(lib, b"mpv_render_context_render\0")?,
            render_context_free: symbol(lib, b"mpv_render_context_free\0")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Os, candidates};
    use std::path::{Path, PathBuf};

    fn names(list: Vec<PathBuf>) -> Vec<String> { list.into_iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect() }

    #[test]
    fn linux_asks_the_loader_for_the_soname_first() {
        assert_eq!(names(candidates(Os::Linux, None, Some(Path::new("/opt/hangar")))), ["libmpv.so.2", "libmpv.so"]);
    }

    #[test]
    fn explicit_path_comes_first_and_empty_is_ignored() {
        let list = names(candidates(Os::Linux, Some("/x/libmpv.so.2".into()), None));
        assert_eq!(list[0], "/x/libmpv.so.2");
        assert_eq!(names(candidates(Os::Linux, Some("".into()), None))[0], "libmpv.so.2");
    }

    #[test]
    fn windows_looks_beside_the_executable_before_the_search_path() {
        assert_eq!(names(candidates(Os::Windows, None, Some(Path::new("C:/Hangar")))),
            ["C:/Hangar/libmpv-2.dll", "C:/Hangar/mpv-2.dll", "libmpv-2.dll", "mpv-2.dll"]);
    }

    #[test]
    fn macos_looks_in_the_bundle_and_in_homebrew() {
        let list = names(candidates(Os::Macos, None, Some(Path::new("/App/Contents/MacOS"))));
        assert_eq!(list[0], "/App/Contents/MacOS/../Frameworks/libmpv.2.dylib");
        assert!(list.contains(&"/opt/homebrew/lib/libmpv.2.dylib".to_owned()));
        assert!(list.contains(&"/usr/local/lib/libmpv.2.dylib".to_owned()));
    }

    #[test]
    fn unknown_system_has_only_the_explicit_path() {
        assert!(candidates(Os::Other, None, None).is_empty());
    }
}
