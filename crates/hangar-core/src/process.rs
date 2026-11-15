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
pub fn resolve_program<'a>(name: &str, env: impl IntoIterator<Item = (&'a String, &'a String)> + Clone) -> Option<PathBuf> {
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
    std::env::split_paths(var("PATH")?).find_map(|dir| {
        exts.iter().map(|ext| dir.join(format!("{name}{ext}"))).find(|p| p.is_file())
    })
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
    fn only_the_cargo_build_folders_count_as_a_test_binary() {
        assert!(in_cargo_build_dir(Path::new("/t/debug/deps/it-0123456789abcdef")));
        assert!(in_cargo_build_dir(Path::new("/t/debug/build/hangar-server/0123456789abcdef/out/it-0123456789abcdef")));
        assert!(!in_cargo_build_dir(Path::new("/t/debug/hangar-server")));
        assert!(!in_cargo_build_dir(Path::new("/home/x/.hangar/current/bin/hangar-server")));
        assert!(!in_cargo_build_dir(Path::new("/srv/out/hangar-server")));
    }
}
