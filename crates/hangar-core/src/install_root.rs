//! Raiz da instalação do Hangar e o Python dela. A mesma regra de `backend/app/install_root.py`.
use std::path::{Path, PathBuf};

pub use crate::paths::current;

/// `~/.hangar/current` quando existe, SEM resolver o link: o que se grava fora do Hangar sobrevive à
/// troca de versão. Senão, a cópia do Hangar acima do binário ou a que o servidor anotou ao subir.
pub fn root(home: &Path) -> Option<PathBuf> {
    let current = current(home);
    if current.exists() { Some(current) } else { checkout(home) }
}

/// O checkout de onde este binário roda, ou o anotado em `~/.hangar/install-root-dir` (ou, na volta,
/// em `~/.hangar/backend-dir`) quando o binário está em `~/.hangar/bin`.
fn checkout(home: &Path) -> Option<PathBuf> {
    checkout_from(std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok()), home)
}

/// Cópia acima do binário com `backend/.env` vence a anotada; sem `.env` (worktree de
/// desenvolvimento) vale a anotada, que é a do servidor e tem o token; sem nenhuma das duas, a cópia
/// acima do binário.
fn checkout_from(exe: Option<PathBuf>, home: &Path) -> Option<PathBuf> {
    use crate::paths::is_install_root;
    let above = |found: &dyn Fn(&Path) -> bool| exe.as_deref()
        .and_then(|exe| exe.ancestors().skip(1).find(|dir| found(dir)).map(Path::to_path_buf));
    above(&|dir: &Path| dir.join("backend").join(".env").is_file())
        .or_else(|| noted_root(home))
        .or_else(|| above(&is_install_root))
}

/// A raiz anotada pelo servidor. O ponteiro novo (a raiz) vence o antigo (a `backend/`), salvo quando
/// o antigo foi gravado depois: a versão anterior subiu na volta e só regrava aquele.
pub fn noted_root(home: &Path) -> Option<PathBuf> {
    use crate::paths::{is_backend, is_install_root};
    let read = |path: PathBuf| {
        let modified = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
        std::fs::read_to_string(&path).ok().map(|text| (PathBuf::from(text.trim()), modified))
    };
    let new = read(crate::paths::root_pointer(home)).filter(|(root, _)| is_install_root(root));
    let old = read(crate::paths::pointer(home))
        .filter(|(backend, _)| is_backend(backend))
        .and_then(|(backend, modified)| Some((backend.parent()?.to_path_buf(), modified)));
    match (new, old) {
        (Some((_, new_at)), Some((old_root, old_at))) if old_at > new_at => Some(old_root),
        (Some((root, _)), _) | (None, Some((root, _))) => Some(root),
        (None, None) => None,
    }
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
    backend_noted(home, args, windows, explicit, &mut Vec::new())
}

/// `backend`, anotando em `notes` (código, causa e o caminho escolhido) cada desvio que ele decide
/// sozinho: o lançador as põe no log da tarefa e no stderr, onde o serviço as guarda.
pub fn backend_noted(home: &Path, args: &[String], windows: bool, explicit: Option<&std::ffi::OsStr>, notes: &mut Vec<String>) -> Result<Backend, (&'static str, PathBuf)> {
    let real = match std::fs::canonicalize(current(home)) {
        Ok(real) => crate::fs::without_verbatim(real),
        Err(error) => {
            let found = checkout(home).ok_or(("launch_no_current", current(home)))?;
            notes.push(format!("launch_checkout: {} indisponível ({:?}); usando o checkout {}", current(home).display(), error.kind(), found.display()));
            found
        }
    };
    let manifest = real.join("manifest.json");
    let direct = match std::fs::read(&manifest).map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes)) {
        Ok(Ok(m)) => m.get("boot").and_then(serde_json::Value::as_str) == Some("direct"),
        // Checkout (modo desenvolvedor) não tem manifesto: sobe pelo Python só enquanto ele existe.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => !real.join("backend").join("app").join("main.py").is_file(),
        // Manifesto ilegível: versão sem o Python embutido só pode subir direto; com ele, não dá para
        // saber qual dos dois, e escolher às cegas viraria um laço de quedas.
        unreadable if !real.join("python").exists() => {
            let cause = match unreadable { Err(e) => format!("{:?}", e.kind()), Ok(_) => "json".to_owned() };
            notes.push(format!("launch_manifest_unreadable_direct: {} ilegível ({cause}); sem o Python embutido, sobe direto", manifest.display()));
            true
        }
        Ok(Err(_)) | Err(_) => return Err(("launch_manifest_unreadable", manifest)),
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
    // Na raiz da versão: o pacote pode não ter `backend/`, e o servidor anota a raiz para o
    // `hangar-server open` de `~/.hangar/bin` achar o `.env`.
    command.current_dir(&real);
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

/// Primeira espera antes de religar o servidor que caiu (decisão 38); dobra a cada queda seguida.
pub const RELAUNCH_FIRST: std::time::Duration = std::time::Duration::from_secs(2);
/// Teto da espera: a partir dele o lançador tenta a cada 10 min, sem desistir.
pub const RELAUNCH_CEILING: std::time::Duration = std::time::Duration::from_secs(600);
/// Vida que durou isto não é queda seguida: a espera volta ao começo. A janela do antigo
/// Supervisor do Python (`CRASH_WINDOW`).
pub const RELAUNCH_STABLE: std::time::Duration = std::time::Duration::from_secs(60);

/// As quedas seguidas do servidor que o lançador religa, e quanto esperar antes de cada religada.
#[derive(Debug, Clone)]
pub struct Relaunch {
    first: std::time::Duration,
    falls: u32,
}

impl Relaunch {
    pub fn new(first: std::time::Duration) -> Self { Self { first, falls: 0 } }

    /// A queda de uma vida que durou `lived`: o número dela entre as seguidas e a espera.
    pub fn fall(&mut self, lived: std::time::Duration) -> (u32, std::time::Duration) {
        if lived >= RELAUNCH_STABLE { self.falls = 0; }
        self.falls = self.falls.saturating_add(1);
        let wait = self.first.saturating_mul(2u32.saturating_pow(self.falls - 1)).min(RELAUNCH_CEILING);
        (self.falls, wait)
    }
}

/// Saída do servidor que é parada pedida, e não queda: 0 é a parada limpa dele (eventos de console
/// no Windows, sinal no Unix); `0xFFFFFFFF` é o `Stop-Process -Force` (`TerminateProcess` com -1) do
/// `Restart-HangarTask`, o reinício normal no Windows. `taskkill /F` encerra com 1: é queda.
pub fn exit_is_requested_stop(code: Option<i32>) -> bool { matches!(code, Some(0 | -1)) }

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
        std::fs::write(worktree.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
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

    /// O ponteiro novo (a raiz) vence o antigo (a `backend/`); o antigo vale sozinho e quando foi
    /// gravado depois, que é o que a versão anterior faz ao subir na volta.
    #[test]
    fn the_root_pointer_wins_unless_the_old_one_is_newer() {
        let dir = tempfile::tempdir().unwrap();
        let (home, new, old) = (dir.path().join("home"), dir.path().join("v2"), dir.path().join("v1"));
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("manifest.json"), "{}").unwrap();
        std::fs::create_dir_all(old.join("backend")).unwrap();
        std::fs::write(old.join("backend/.env"), "CP_AUTH_TOKEN=t\n").unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        let exe = Some(dir.path().join("elsewhere/hangar-server"));
        assert_eq!(super::noted_root(&home), None);
        std::fs::write(crate::paths::pointer(&home), old.join("backend").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(checkout_from(exe.clone(), &home), Some(old.clone()), "só o antigo: vale ele");
        let at = |path: std::path::PathBuf, secs: u64| std::fs::File::options().write(true).open(path).unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        std::fs::write(crate::paths::root_pointer(&home), new.to_string_lossy().as_bytes()).unwrap();
        at(crate::paths::pointer(&home), 1_000);
        at(crate::paths::root_pointer(&home), 2_000);
        assert_eq!(checkout_from(exe.clone(), &home), Some(new.clone()), "o novo vence");
        at(crate::paths::pointer(&home), 3_000);
        assert_eq!(checkout_from(exe.clone(), &home), Some(old), "a volta regravou só o antigo");
        // Raiz anotada que deixou de ser uma cópia do Hangar não vale.
        std::fs::remove_file(crate::paths::pointer(&home)).unwrap();
        std::fs::remove_file(new.join("manifest.json")).unwrap();
        assert_eq!(checkout_from(exe, &home), None);
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
        // A versão que sobe direto não precisa da `backend/`: o servidor roda na raiz dela.
        std::fs::remove_dir(real.join("backend")).unwrap();
        let args = ["--log".to_owned(), "/tmp/x.log".to_owned()];
        let Ok(super::Backend::Direct(command)) = super::backend(home, &args, false, None) else { panic!("devia ser direto") };
        assert_eq!(command.get_program(), real.join("bin/hangar-server"));
        assert_eq!(command.get_args().count(), 0, "o --log fica com o lançador");
        assert_eq!(command.get_current_dir(), Some(real.as_path()));
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

    /// Manifesto ilegível: sem o Python embutido, só pode ser direto; com ele, recusa com código
    /// em vez de escolher às cegas.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_manifest_never_guesses_the_python() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = version(home, "1.0.0.9", "{ quebrado");
        assert_eq!(super::backend(home, &[], false, None).err(), Some(("launch_manifest_unreadable", real.join("manifest.json"))));
        std::fs::remove_dir_all(real.join("python")).unwrap();
        let mut notes = Vec::new();
        assert!(matches!(super::backend_noted(home, &[], false, None, &mut notes), Ok(super::Backend::Direct(_))));
        // A escolha às cegas fica dita, com o caminho e a causa.
        assert_eq!(notes, [format!("launch_manifest_unreadable_direct: {} ilegível (json); sem o Python embutido, sobe direto",
            real.join("manifest.json").display())]);
        let mut clean = Vec::new();
        std::fs::write(real.join("manifest.json"), r#"{"boot":"direct"}"#).unwrap();
        assert!(matches!(super::backend_noted(home, &[], false, None, &mut clean), Ok(super::Backend::Direct(_))));
        assert!(clean.is_empty(), "{clean:?}");
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

    /// Decisão 38: 2 s dobrando até o teto de 10 min, sem desistir; uma vida de 60 s ou mais volta
    /// a espera ao começo.
    #[test]
    fn the_relaunch_wait_doubles_up_to_ten_minutes_and_resets_after_a_stable_life() {
        use std::time::Duration;
        let mut relaunch = super::Relaunch::new(super::RELAUNCH_FIRST);
        let waits: Vec<_> = (0..12).map(|_| relaunch.fall(Duration::from_secs(1))).collect();
        let secs: Vec<_> = waits.iter().map(|(_, wait)| wait.as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 64, 128, 256, 512, 600, 600, 600]);
        assert_eq!(waits.last().unwrap().0, 12, "conta as quedas seguidas");
        for _ in 0..100 { relaunch.fall(Duration::ZERO); }
        assert_eq!(relaunch.fall(Duration::ZERO).1, super::RELAUNCH_CEILING, "sem estouro depois de muitas quedas");
        assert_eq!(relaunch.fall(Duration::from_secs(59)).1, super::RELAUNCH_CEILING, "59 s ainda é queda seguida");
        assert_eq!(relaunch.fall(super::RELAUNCH_STABLE), (1, Duration::from_secs(2)), "vida estável recomeça a conta");
        assert_eq!(relaunch.fall(Duration::ZERO), (2, Duration::from_secs(4)));
        let mut quick = super::Relaunch::new(Duration::from_millis(200));
        assert_eq!([quick.fall(Duration::ZERO).1, quick.fall(Duration::ZERO).1].map(|w| w.as_millis()), [200, 400]);
    }

    /// Parada pedida não se religa: a limpa (0) e o `Stop-Process` do reinício (`0xFFFFFFFF`).
    #[test]
    fn only_a_clean_exit_or_the_restart_stop_process_is_a_requested_stop() {
        assert!(super::exit_is_requested_stop(Some(0)));
        assert!(super::exit_is_requested_stop(Some(0xFFFF_FFFFu32 as i32)));
        for fall in [Some(1), Some(2), Some(5), Some(101), Some(0xC000_0409u32 as i32), None] {
            assert!(!super::exit_is_requested_stop(fall), "{fall:?}");
        }
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
