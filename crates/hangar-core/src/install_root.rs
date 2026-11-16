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

/// O que o lançador sobe.
#[derive(Debug)]
pub enum Backend {
    /// Versão que ainda sobe pelo Python (`-m app.main`): pacote sem `boot: direct`, inclusive na volta.
    Python(std::process::Command),
    /// O `hangar-server` sozinho.
    Direct(std::process::Command),
}

/// O backend da versão em uso: pelo `current` (caminho real) ou, sem ele, pelo checkout do lançador.
/// O manifesto decide (`boot: direct`); checkout sem manifesto sobe pelo Python só enquanto ele existe.
pub fn backend(home: &Path, args: &[String], windows: bool, explicit: Option<&std::ffi::OsStr>) -> Result<Backend, (&'static str, PathBuf)> {
    let real = match std::fs::canonicalize(current(home)) {
        Ok(real) => crate::fs::without_verbatim(real),
        Err(_) => checkout(home).ok_or(("launch_no_current", current(home)))?,
    };
    let direct = match std::fs::read(real.join("manifest.json")) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).ok()
            .is_some_and(|m| m.get("boot").and_then(serde_json::Value::as_str) == Some("direct")),
        Err(_) => !real.join("backend").join("app").join("main.py").is_file(),
    };
    if !direct {
        let python = python_exe(&real, windows);
        if !python.is_file() { return Err(("launch_no_python", python)); }
        let mut command = std::process::Command::new(python);
        command.args(["-m", "app.main"]).args(args).current_dir(real.join("backend"));
        return Ok(Backend::Python(command));
    }
    let binary = server_binary(home, &real, explicit, windows)?;
    let mut command = std::process::Command::new(binary);
    // `--log` é do lançador, que abre o arquivo e o entrega como saída do servidor.
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--log" { rest.next(); } else { command.arg(arg); }
    }
    // Na `backend/` o servidor anota a pasta para o `hangar-server open` de `~/.hangar/bin` achar o `.env`.
    command.current_dir(real.join("backend"));
    Ok(Backend::Direct(command))
}

/// `CP_RUST_SERVER_BIN` explícito vence e, errado, não cai para outro; senão o `bin/` da versão;
/// senão o mais novo entre o build do checkout e `~/.hangar/bin` (empate: o checkout).
pub fn server_binary(home: &Path, real: &Path, explicit: Option<&std::ffi::OsStr>, windows: bool) -> Result<PathBuf, (&'static str, PathBuf)> {
    let exe = if windows { "hangar-server.exe" } else { "hangar-server" };
    if let Some(explicit) = explicit.filter(|e| !e.is_empty()) {
        let path = PathBuf::from(explicit);
        return if runnable(&path) { Ok(path) } else { Err(("launch_server_bin_invalid", path)) };
    }
    let packaged = real.join("bin").join(exe);
    if runnable(&packaged) { return Ok(packaged); }
    let modified = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    [real.join("crates").join("target").join("release").join(exe), home.join(".hangar").join("bin").join(exe)]
        .into_iter().filter(|p| runnable(p))
        .reduce(|newest, other| if modified(&other) > modified(&newest) { other } else { newest })
        .ok_or(("launch_no_server", packaged))
}

fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) }
    #[cfg(not(unix))]
    { path.is_file() }
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

    #[cfg(unix)]
    fn executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Uma versão do pacote com o manifesto dado, o Python embutido e o `bin/hangar-server`.
    #[cfg(unix)]
    fn version(home: &std::path::Path, name: &str, manifest: &str) -> std::path::PathBuf {
        let real = home.join(".hangar/versions").join(name);
        std::fs::create_dir_all(real.join("backend")).unwrap();
        std::fs::write(real.join("manifest.json"), manifest).unwrap();
        executable(&real.join("python/bin/python3"));
        executable(&real.join("bin/hangar-server"));
        let current = current(home);
        let _ = std::fs::remove_file(&current);
        std::os::unix::fs::symlink(&real, &current).unwrap();
        real.canonicalize().unwrap()
    }

    /// B-18, decisão 1: `boot: direct` abre o `hangar-server` da versão, sem o `--log` do lançador.
    #[cfg(unix)]
    #[test]
    fn a_direct_version_launches_its_own_server_without_python() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = version(home, "1.0.0.9", r#"{"version":"1.0.0.9","boot":"direct"}"#);
        let args = ["--log".to_owned(), "/tmp/x.log".to_owned()];
        let Ok(super::Backend::Direct(command)) = super::backend(home, &args, false, None) else { panic!("devia ser direto") };
        assert_eq!(command.get_program(), real.join("bin/hangar-server"));
        assert_eq!(command.get_args().count(), 0, "o --log fica com o lançador");
        assert_eq!(command.get_current_dir(), Some(real.join("backend").as_path()));
    }

    /// Decisão 1: versão sem o campo (a anterior, na volta da atualização) sobe pelo Python como antes.
    #[cfg(unix)]
    #[test]
    fn a_version_without_boot_still_launches_the_python() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = version(home, "1.0.0.8", r#"{"version":"1.0.0.8"}"#);
        let args = ["--log".to_owned(), "/tmp/x.log".to_owned()];
        let Ok(super::Backend::Python(command)) = super::backend(home, &args, false, None) else { panic!("devia ser o Python") };
        assert_eq!(command.get_program(), real.join("python/bin/python3"));
        let got: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(got, ["-m", "app.main", "--log", "/tmp/x.log"], "o Python abre o próprio log");
    }

    /// B-18, decisão 7: o explícito vence e, errado, não cai para o binário da versão.
    #[cfg(unix)]
    #[test]
    fn the_explicit_binary_wins_and_a_wrong_one_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        version(home, "1.0.0.9", r#"{"boot":"direct"}"#);
        let mine = home.join("mine/hangar-server");
        executable(&mine);
        let Ok(super::Backend::Direct(command)) = super::backend(home, &[], false, Some(mine.as_os_str())) else { panic!() };
        assert_eq!(command.get_program(), mine.as_os_str());
        let wrong = home.join("nope/hangar-server");
        assert_eq!(super::backend(home, &[], false, Some(wrong.as_os_str())).err(), Some(("launch_server_bin_invalid", wrong)));
    }

    /// B-18: checkout (modo desenvolvedor) sem `bin/`: o mais novo entre o build dele e `~/.hangar/bin`;
    /// enquanto o checkout tiver o `app/main.py`, sobe pelo Python.
    #[cfg(unix)]
    #[test]
    fn a_checkout_uses_the_newest_built_server_once_the_python_boot_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(checkout.join("backend/app")).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::os::unix::fs::symlink(&checkout, current(&home)).unwrap();
        executable(&checkout.join("backend/.venv/bin/python"));
        std::fs::write(checkout.join("backend/app/main.py"), "").unwrap();
        assert!(matches!(super::backend(&home, &[], false, None), Ok(super::Backend::Python(_))));
        std::fs::remove_file(checkout.join("backend/app/main.py")).unwrap();
        assert_eq!(super::backend(&home, &[], false, None).err().map(|e| e.0), Some("launch_no_server"));
        let (built, installed) = (checkout.join("crates/target/release/hangar-server"), home.join(".hangar/bin/hangar-server"));
        executable(&built);
        executable(&installed);
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&built).unwrap().set_modified(older).unwrap();
        let program = |home: &std::path::Path| match super::backend(home, &[], false, None) {
            Ok(super::Backend::Direct(c)) => std::path::PathBuf::from(c.get_program()),
            other => panic!("{other:?}"),
        };
        assert_eq!(program(&home), installed, "o de ~/.hangar/bin é mais novo");
        std::fs::File::options().write(true).open(&installed).unwrap().set_modified(older - std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(program(&home), checkout.canonicalize().unwrap().join("crates/target/release/hangar-server"));
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
