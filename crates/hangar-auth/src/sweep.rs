//! Pastas `hangar-auth-*` que sobraram de um backend morto sem saída limpa.
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Nome das pastas que o backend cria para o contador compartilhado.
pub const PREFIX: &str = "hangar-auth-";
/// Marcador de vida: cada `SharedLimiter` aberto segura uma trava compartilhada nele até fechar.
pub const ALIVE: &str = "state.alive";
/// Folga entre o backend criar o marcador e travá-lo; bem acima de qualquer subida.
pub const MIN_AGE: Duration = Duration::from_secs(6 * 3600);

/// `skipped`: pastas com o prefixo que não deu para comparar com a nossa (ou entradas ilegíveis da raiz) e por isso
/// ficaram. `first_failure`: a primeira órfã que não saiu, para o log dizer qual e por quê.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sweep { pub removed: usize, pub failed: usize, pub skipped: usize,
    pub first_failure: Option<(std::path::PathBuf, std::io::ErrorKind)> }

/// Apaga as pastas órfãs direto em `root`. Pasta sem marcador nunca sai: nada prova que o dono morreu.
/// `current` é a pasta do contexto deste processo; na dúvida sobre qualquer pasta, ela fica.
#[cfg(unix)]
pub fn sweep_orphans(root: &Path, current: Option<&Path>, now: SystemTime) -> std::io::Result<Sweep> {
    use std::fs;
    let current = current.map(fs::canonicalize);
    let mut outcome = Sweep::default();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { outcome.skipped += 1; continue };
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) { continue; }
        let dir = entry.path();
        // Sem seguir link: só pasta de verdade, e só arquivo de verdade como marcador.
        if !fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_dir()) { continue; }
        match (&current, fs::canonicalize(&dir)) {
            (Some(Ok(mine)), Ok(found)) if *mine == found => continue,
            (Some(Ok(_)) | None, Ok(_)) => {}
            // Sem saber qual é a nossa, ou sem resolver esta: fica, e conta.
            _ => { outcome.skipped += 1; continue; }
        }
        let marker = dir.join(ALIVE);
        let Ok(meta) = fs::symlink_metadata(&marker) else { continue };
        let aged = meta.modified().ok().and_then(|at| now.duration_since(at).ok()).is_some_and(|age| age >= MIN_AGE);
        if !meta.file_type().is_file() || !aged { continue; }
        // A trava exclusiva só passa sem nenhum limitador vivo na pasta; fica presa até a pasta sair. Um filho
        // recém-criado de outro processo pode segurá-la por um instante: a órfã espera a próxima subida.
        let Ok(alive) = fs::File::open(&marker) else { continue };
        if alive.try_lock().is_err() { continue; }
        match fs::remove_dir_all(&dir) {
            Ok(()) => outcome.removed += 1,
            Err(error) => { outcome.failed += 1; outcome.first_failure.get_or_insert((dir, error.kind())); }
        }
    }
    Ok(outcome)
}

/// Fora do Unix nada é varrido: o marcador não existe lá.
#[cfg(not(unix))]
pub fn sweep_orphans(_root: &Path, _current: Option<&Path>, _now: SystemTime) -> std::io::Result<Sweep> {
    Ok(Sweep::default())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{AuthAttempt, AuthConfig, Profile, SharedLimiter, AUTH_ABI, AUTH_STORE};
    use std::fs::{self, File};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    /// Em fila: entre o `fork` e o `exec` do teste que cria um processo, o filho segura cópia de todo marcador
    /// aberto, e a trava dele não solta no `drop` de quem o abriu.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn now() -> SystemTime { UNIX_EPOCH + Duration::from_secs(2_000_000_000) }
    fn old() -> Duration { MIN_AGE + Duration::from_secs(3600) }

    fn folder(root: &Path, name: &str, marker_age: Option<Duration>) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("state.json"), b"{}").unwrap();
        if let Some(age) = marker_age { age_marker(&dir, age); }
        dir
    }
    fn age_marker(dir: &Path, age: Duration) {
        let marker = fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join(ALIVE)).unwrap();
        marker.set_modified(now() - age).unwrap();
    }
    fn config(dir: &Path, mode: &str) -> AuthConfig {
        AuthConfig { abi: AUTH_ABI, store_version: AUTH_STORE, mode: mode.into(), state_path: dir.join("state.json"),
            parent_epoch: "a".repeat(32), expected_token: "owner".into(), receipt_key: "00".repeat(32) }
    }

    #[test]
    fn an_unresolved_current_keeps_every_directory_and_counts_it() {
        let _serial = serial();
        let root = tempfile::tempdir().unwrap();
        let orphan = folder(root.path(), "hangar-auth-orfa", Some(old()));
        let outcome = sweep_orphans(root.path(), Some(&root.path().join("sumiu")), now()).unwrap();
        assert_eq!(outcome, Sweep { skipped: 1, ..Sweep::default() });
        assert!(orphan.exists(), "sem saber qual pasta é a nossa, a órfã saiu");
    }

    #[test]
    fn only_an_old_marked_directory_nobody_holds_is_removed() {
        let _serial = serial();
        let base = tempfile::tempdir().unwrap();
        let (root, outside) = (base.path().join("hangar"), base.path().join("outside"));
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let orphan = folder(&root, "hangar-auth-orphan", Some(old()));
        let held = folder(&root, "hangar-auth-held", Some(old()));
        let holder = File::open(held.join(ALIVE)).unwrap();
        holder.lock_shared().unwrap();
        let current = folder(&root, "hangar-auth-current", Some(old()));
        let recent = folder(&root, "hangar-auth-recent", Some(MIN_AGE - Duration::from_secs(1)));
        let legacy = folder(&root, "hangar-auth-legacy", None);
        let other = folder(&root, "outro-prefixo", Some(old()));
        let target = folder(&outside, "hangar-auth-target", Some(old()));
        std::os::unix::fs::symlink(&target, root.join("hangar-auth-link")).unwrap();

        let outcome = sweep_orphans(&root, Some(&current), now()).unwrap();

        assert!(!orphan.exists(), "a órfã velha com marcador ficou");
        assert!(held.join("state.json").exists(), "saiu a pasta com o marcador travado por outro descritor");
        assert!(current.join("state.json").exists(), "saiu a pasta do contexto atual");
        assert!(recent.join("state.json").exists(), "saiu a pasta recente");
        assert!(legacy.join("state.json").exists(), "saiu a pasta sem marcador");
        assert!(other.join("state.json").exists(), "saiu a pasta de outro prefixo");
        assert!(target.join("state.json").exists(), "o link simbólico foi seguido");
        assert!(fs::symlink_metadata(root.join("hangar-auth-link")).is_ok(), "o link simbólico saiu");
        assert_eq!(outcome, Sweep { removed: 1, ..Sweep::default() });
        drop(holder);
    }

    #[test]
    fn an_open_limiter_holds_the_marker_until_it_is_dropped() {
        let _serial = serial();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hangar-auth-live");
        fs::create_dir(&dir).unwrap();
        let owner = SharedLimiter::open(config(&dir, "create")).unwrap();
        assert!(dir.join(ALIVE).is_file(), "abrir o limitador não criou o marcador de vida");
        age_marker(&dir, old());
        assert_eq!(sweep_orphans(root.path(), None, now()).unwrap(), Sweep::default(), "saiu a pasta com o dono vivo");
        let attached = SharedLimiter::open(config(&dir, "attach")).unwrap();
        drop(owner);
        assert_eq!(sweep_orphans(root.path(), None, now()).unwrap(), Sweep::default(), "saiu a pasta com quem anexou vivo");
        drop(attached);
        assert_eq!(sweep_orphans(root.path(), None, now()).unwrap(), Sweep { removed: 1, ..Sweep::default() });
        assert!(!dir.exists());
    }

    #[test]
    fn a_held_marker_does_not_keep_the_directory_from_a_clean_removal() {
        let _serial = serial();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hangar-auth-clean");
        fs::create_dir(&dir).unwrap();
        let owner = SharedLimiter::open(config(&dir, "create")).unwrap();
        let attached = SharedLimiter::open(config(&dir, "attach")).unwrap();
        drop(owner);
        // A saída limpa do backend apaga a pasta com o filho ainda anexado.
        fs::remove_dir_all(&dir).unwrap();
        assert!(!dir.exists());
        drop(attached);
    }

    #[test]
    fn a_marker_that_cannot_be_opened_leaves_the_limiter_working() {
        let _serial = serial();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hangar-auth-blocked");
        fs::create_dir(&dir).unwrap();
        fs::create_dir(dir.join(ALIVE)).unwrap();
        let limiter = SharedLimiter::open(config(&dir, "create")).unwrap();
        let decision = limiter.evaluate(&AuthAttempt { profile: Profile::RequireAuthHttp, origin: "198.51.100.7".into(),
            presented_token: Some(b"owner".to_vec()), verified_guest: false, login_guest: false }).unwrap();
        assert_eq!(decision, crate::AuthDecision::OWNER);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_marker_descriptor_does_not_reach_child_processes() {
        let _serial = serial();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hangar-auth-child");
        fs::create_dir(&dir).unwrap();
        let _owner = SharedLimiter::open(config(&dir, "create")).unwrap();
        let listed = std::process::Command::new("/bin/sh")
            .args(["-c", "for f in /proc/$$/fd/*; do readlink \"$f\"; done"]).output().unwrap();
        let open = String::from_utf8_lossy(&listed.stdout);
        assert!(listed.status.success() && !open.is_empty(), "o filho não listou os descritores");
        assert!(!open.contains(ALIVE), "o marcador vazou para o processo filho: {open}");
    }
}
