//! Erros de processo que não são falha e a busca do executável pelo PATH.
use std::io;
use std::path::{Path, PathBuf};

/// Saída capturada de um comando local, sem interpretação de shell.
pub struct CommandOutput { pub success: bool, pub stdout: String, pub stderr: String }

#[derive(Debug)]
pub enum CommandError { Timeout, Io(io::Error) }

/// Fronteira dos executáveis externos; a política e os argumentos continuam no consumidor.
pub trait Commands: Send + Sync {
    fn run<'a>(&'a self, program: &'a str, args: &'a [&'a str], deadline: std::time::Duration)
        -> futures_util::future::BoxFuture<'a, Result<CommandOutput, CommandError>>;
}


/// Binário de teste: o `cargo test` põe todos em `target/<perfil>/build/<pacote>/<hash>/out/` (em
/// `target/<perfil>/deps/` no layout antigo), e o teste de integração compila os crates sem
/// `cfg(test)`. Nele nada pode chegar ao que é da máquina real.
pub fn cargo_test_binary() -> bool {
    std::env::current_exe().is_ok_and(|exe| in_cargo_build_dir(&exe))
}

fn in_cargo_build_dir(exe: &Path) -> bool {
    let dir = |up: usize| exe.ancestors().nth(up).and_then(Path::file_name);
    dir(1).is_some_and(|name| name == "deps") || (dir(1).is_some_and(|name| name == "out") && dir(4).is_some_and(|name| name == "build"))
}

/// O executável pelo PATH do ambiente da sessão (PATHEXT no Windows: o `hangar-engine` é `.CMD`).
/// No Windows, sem achar ali, procura também no PATH atual do registro: o instalador de um CLI grava a
/// pasta dele lá, e o ambiente capturado na subida do servidor não a vê até reiniciar.
pub fn resolve_program<'a>(name: &str, env: impl IntoIterator<Item = (&'a String, &'a String)> + Clone) -> Option<PathBuf> {
    #[cfg(windows)]
    let installed = || (!cargo_test_binary()).then(registry_path).flatten();
    #[cfg(not(windows))]
    let installed = || None;
    resolve_with(name, env, installed)
}

fn resolve_with<'a>(
    name: &str,
    env: impl IntoIterator<Item = (&'a String, &'a String)> + Clone,
    installed: impl FnOnce() -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 { return path.is_file().then(|| path.to_path_buf()); }
    let var = |k: &str| env.clone().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
    // No Windows o nome cru só vale se já termina numa extensão do PATHEXT, como o `shutil.which`:
    // `codex` sem extensão é o shim de shell do npm, que o CreateProcess não roda.
    let exts: Vec<String> = if cfg!(windows) {
        let pathext: Vec<String> = var("PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD").split(';')
            .filter(|e| !e.is_empty()).map(str::to_ascii_lowercase).collect();
        let lower = name.to_ascii_lowercase();
        if pathext.iter().any(|e| lower.ends_with(e.as_str())) { vec![String::new()] } else { pathext }
    } else { vec![String::new()] };
    let find = |path: &std::ffi::OsStr| std::env::split_paths(path).find_map(|dir| {
        exts.iter().map(|ext| dir.join(format!("{name}{ext}"))).find(|p| p.is_file())
    });
    let path = var("PATH")?;
    // PATH vazio é "nenhum programa" de propósito; o registro só completa um PATH de verdade.
    find(path.as_ref()).or_else(|| (!path.is_empty()).then(installed).flatten().and_then(|p| find(&p)))
}

/// PATH da máquina seguido do PATH do usuário, como o Windows monta para um processo novo.
#[cfg(windows)]
#[allow(unsafe_code)] // RegGetValueW não tem equivalente no std.
fn registry_path() -> Option<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let value = wide("Path");
    // Sem RRF_NOEXPAND o REG_EXPAND_SZ já volta com %VAR% expandido.
    let read = |root: HKEY, key: &str| -> Option<Vec<u16>> {
        let key = wide(key);
        let mut buffer = vec![0u16; 2048];
        for _ in 0..4 {
            let mut size = (buffer.len() * 2) as u32;
            let code = unsafe {
                RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size)
            };
            match code {
                ERROR_SUCCESS => {
                    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                    buffer.truncate(len);
                    return Some(buffer);
                }
                ERROR_MORE_DATA => buffer.resize((size as usize).div_ceil(2).max(buffer.len() * 2), 0),
                _ => return None,
            }
        }
        None
    };
    let parts: Vec<Vec<u16>> = [
        read(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        read(HKEY_CURRENT_USER, "Environment"),
    ].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| std::ffi::OsString::from_wide(&parts.join(&(';' as u16))))
}

/// Processo que morreu entre a varredura e a leitura: não é falha, é corrida.
pub fn vanished(error: &io::Error) -> bool {
    // ESRCH: o `/proc/<pid>` ainda existe, o processo já não.
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_binary_is_recognized_in_the_layout_cargo_gave_it() {
        assert!(cargo_test_binary(), "{}", std::env::current_exe().unwrap().display());
    }

    #[test]
    fn a_program_installed_after_the_capture_is_found_in_the_current_path() {
        let (captured, installed) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let program = if cfg!(windows) { "agy.exe" } else { "agy" };
        std::fs::write(installed.path().join(program), "").unwrap();
        let env = [("PATH".to_owned(), captured.path().to_string_lossy().into_owned()), ("PATHEXT".to_owned(), ".EXE".to_owned())];
        let env = env.iter().map(|(k, v)| (k, v));
        let current = || Some(installed.path().as_os_str().to_owned());
        assert_eq!(resolve_with("agy", env.clone(), current), Some(installed.path().join(program)));
        assert_eq!(resolve_with("agy", env, || None), None);
        let empty = [("PATH".to_owned(), String::new())];
        assert_eq!(resolve_with("agy", empty.iter().map(|(k, v)| (k, v)), current), None, "PATH vazio não procura no registro");
    }

    #[test]
    fn only_the_cargo_build_folders_count_as_a_test_binary() {
        assert!(in_cargo_build_dir(Path::new("/t/debug/deps/it-0123456789abcdef")));
        assert!(in_cargo_build_dir(Path::new("/t/debug/build/hangar-server/0123456789abcdef/out/it-0123456789abcdef")));
        assert!(!in_cargo_build_dir(Path::new("/t/debug/hangar-server")));
        assert!(!in_cargo_build_dir(Path::new("/home/x/.hangar/current/bin/hangar-server")));
        assert!(!in_cargo_build_dir(Path::new("/srv/out/hangar-server")));
    }
}
