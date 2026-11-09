//! Raiz da instalação do Hangar e o Python dela. A mesma regra de `backend/app/install_root.py`.
use std::path::{Path, PathBuf};

pub use crate::paths::current;

/// `~/.hangar/current` quando existe, SEM resolver o link: o que se grava fora do Hangar sobrevive à
/// troca de versão. Senão, a cópia do Hangar acima do binário ou a que o servidor anotou ao subir.
pub fn root(home: &Path) -> Option<PathBuf> {
    let current = current(home);
    if current.exists() { Some(current) } else { checkout(home) }
}

/// O checkout de onde este binário roda, ou o anotado em `~/.hangar/backend-dir` (binário em `~/.hangar/bin`).
fn checkout(home: &Path) -> Option<PathBuf> {
    checkout_from(std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok()), home)
}

/// Cópia acima do binário com `.env` vence a anotada; sem `.env` (worktree de desenvolvimento) vale a
/// anotada, que é a do servidor e tem o token; sem nenhuma das duas, a cópia acima do binário.
fn checkout_from(exe: Option<PathBuf>, home: &Path) -> Option<PathBuf> {
    use crate::paths::is_backend;
    let above = |found: fn(&Path) -> bool| exe.as_deref()
        .and_then(|exe| exe.ancestors().skip(1).map(|dir| dir.join("backend")).find(|dir| found(dir)));
    let backend = above(|dir| dir.join(".env").is_file())
        .or_else(|| {
            let noted = PathBuf::from(std::fs::read_to_string(crate::paths::pointer(home)).ok()?.trim());
            is_backend(&noted).then_some(noted)
        })
        .or_else(|| above(is_backend))?;
    backend.parent().map(Path::to_path_buf)
}

/// O Python embutido do pacote; sem ele, o venv do backend (checkout ou modo desenvolvedor).
pub fn python_exe(root: &Path, windows: bool) -> PathBuf {
    let venv = root.join("backend").join(".venv");
    let (embedded, venv) = if windows {
        (root.join("python").join("python.exe"), venv.join("Scripts").join("python.exe"))
    } else {
        (root.join("python").join("bin").join("python3"), venv.join("bin").join("python"))
    };
    if embedded.exists() { embedded } else { venv }
}

/// O backend da versão para onde o `current` aponta agora, pelo caminho real: o processo de pé nunca
/// carrega arquivo da versão seguinte depois da troca. O erro leva o caminho que faltou.
pub fn backend_command(home: &Path, args: &[String], windows: bool) -> Result<std::process::Command, (&'static str, PathBuf)> {
    let no_current = || ("launch_no_current", current(home));
    let real = std::fs::canonicalize(current(home)).map_err(|_| no_current())?;
    // O Python não entende o `\\?\` que o `canonicalize` do Windows devolve.
    let real = crate::fs::without_verbatim(real);
    let python = python_exe(&real, windows);
    if !python.is_file() { return Err(("launch_no_python", python)); }
    let mut command = std::process::Command::new(python);
    // Sem `PYTHONPYCACHEPREFIX`: o pacote leva os `.pyc` em `unchecked-hash`, que o Python só lê, e o
    // prefixo os faria ignorar e recompilar tudo fora da versão.
    command.args(["-m", "app.main"]).args(args).current_dir(real.join("backend"));
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::{checkout_from, current, python_exe};

    /// Binário de uma worktree sem `.env` (máquina não migrada): vale a cópia que o servidor anotou,
    /// que tem o token, e não a worktree.
    #[test]
    fn a_worktree_binary_without_env_uses_the_noted_backend() {
        let dir = tempfile::tempdir().unwrap();
        let (home, worktree, main) = (dir.path().join("home"), dir.path().join("wt"), dir.path().join("main"));
        let exe = worktree.join("crates/target/release/hangar-server");
        std::fs::create_dir_all(worktree.join("backend/app")).unwrap();
        std::fs::write(worktree.join("backend/app/rust_server.py"), "").unwrap();
        std::fs::create_dir_all(main.join("backend")).unwrap();
        std::fs::write(main.join("backend/.env"), "CP_AUTH_TOKEN=t\n").unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::fs::write(crate::paths::pointer(&home), main.join("backend").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(checkout_from(Some(exe.clone()), &home), Some(main.clone()));
        // A worktree com `.env` próprio é a do binário, como antes.
        std::fs::write(worktree.join("backend/.env"), "CP_AUTH_TOKEN=w\n").unwrap();
        assert_eq!(checkout_from(Some(exe.clone()), &home), Some(worktree.clone()));
        // Sem nada anotado, a cópia acima do binário ainda vale sem `.env`.
        std::fs::remove_file(worktree.join("backend/.env")).unwrap();
        std::fs::remove_file(crate::paths::pointer(&home)).unwrap();
        assert_eq!(checkout_from(Some(exe), &home), Some(worktree));
    }

    /// O pacote leva os `.pyc` prontos: com `PYTHONPYCACHEPREFIX` o Python os ignora e recompila tudo.
    #[cfg(unix)]
    #[test]
    fn the_launcher_uses_the_shipped_bytecode() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = home.join(".hangar/versions/1.0.0.5");
        std::fs::create_dir_all(real.join("python/bin")).unwrap();
        std::fs::write(real.join("python/bin/python3"), "").unwrap();
        std::os::unix::fs::symlink(&real, current(home)).unwrap();
        let command = super::backend_command(home, &[], false).unwrap();
        assert!(command.get_envs().all(|(key, _)| key != "PYTHONPYCACHEPREFIX"), "{command:?}");
    }

    #[test]
    fn install_root_python_is_the_embedded_one_then_the_venv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(python_exe(root, false), root.join("backend/.venv/bin/python"));
        assert_eq!(python_exe(root, true), root.join("backend").join(".venv").join("Scripts").join("python.exe"));
        for (windows, embedded) in [(false, root.join("python/bin/python3")), (true, root.join("python").join("python.exe"))] {
            std::fs::create_dir_all(embedded.parent().unwrap()).unwrap();
            std::fs::write(&embedded, "").unwrap();
            assert_eq!(python_exe(root, windows), embedded, "windows={windows}");
        }
    }
}
