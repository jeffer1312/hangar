//! Erros de processo que não são falha e a busca do executável pelo PATH.
use std::io;
use std::path::{Path, PathBuf};

/// Binário de teste: o `cargo test` põe todos em `target/<perfil>/deps/`, e o teste de integração
/// compila os crates sem `cfg(test)`. Nele nada pode chegar ao que é da máquina real.
pub fn cargo_test_binary() -> bool {
    std::env::current_exe().ok().is_some_and(|exe| exe.parent().and_then(|dir| dir.file_name()).is_some_and(|name| name == "deps"))
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
