//! Variáveis de tela para o que o servidor lança. O serviço do usuário pode subir antes de o compositor exportá-las, e aí o
//! processo filho nasce sem tela: o controle de tela recusa, programa gráfico não abre janela.
use std::path::{Path, PathBuf};

pub const KEYS: [&str; 4] = ["WAYLAND_DISPLAY", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "XDG_RUNTIME_DIR"];

/// As variáveis de `KEYS` que faltam em `current`, vindas do ambiente do gerenciador systemd do usuário (`systemd`, a saída
/// de `systemctl --user show-environment`) e, sem ele, da pasta de runtime: o primeiro socket `wayland-*` e a instância do
/// Hyprland mais recente em `hypr/`. O que o processo já tem só é trocado quando aponta para socket ou instância que não
/// existe mais.
pub fn missing(current: impl Fn(&str) -> Option<String>, systemd: &str, runtime: Option<&Path>) -> Vec<(String, String)> {
    // Valor (do processo ou do gerenciador) que aponta para socket ou instância que não existe mais (Hyprland reiniciado)
    // não vale.
    let alive = |key: &str, value: &str| match (key, runtime) {
        ("WAYLAND_DISPLAY", Some(run)) => run.join(value).exists(),
        ("HYPRLAND_INSTANCE_SIGNATURE", Some(run)) => run.join("hypr").join(value).exists(),
        _ => true,
    };
    let from_systemd = |key: &str| systemd.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).filter(|v| !v.is_empty())
        .filter(|v| alive(key, v)).map(str::to_owned);
    let from_runtime = |key: &str| match key {
        "WAYLAND_DISPLAY" => names(runtime?).into_iter().filter(|(n, _)| n.starts_with("wayland-") && !n.ends_with(".lock"))
            .map(|(n, _)| n).min(),
        "HYPRLAND_INSTANCE_SIGNATURE" => names(&runtime?.join("hypr")).into_iter().max_by_key(|(_, at)| *at).map(|(n, _)| n),
        _ => None,
    };
    KEYS.iter().filter(|key| current(key).filter(|v| alive(key, v)).is_none())
        .filter_map(|key| from_systemd(key).or_else(|| from_runtime(key)).map(|v| ((*key).to_owned(), v))).collect()
}

/// Nome e data de modificação de cada entrada de `dir`; pasta ilegível = nenhuma.
fn names(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.metadata().ok()?.modified().ok()?))).collect()
}

/// Todas as de `KEYS` para o que se lança: as de `current` e, as que faltam ou morreram, as de `missing`.
pub fn complete(current: impl Fn(&str) -> Option<String>, systemd: &str, runtime: Option<&Path>) -> Vec<(String, String)> {
    let found = missing(&current, systemd, runtime);
    KEYS.iter().filter_map(|key| found.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).or_else(|| current(key))
        .map(|v| ((*key).to_owned(), v))).collect()
}

/// `complete` no Linux, com o gerenciador do usuário lido agora só se faltar alguma.
pub fn for_launch(current: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    if !cfg!(target_os = "linux") { return complete(current, "", None); }
    let (systemd, runtime) = if KEYS.iter().all(|key| current(key).is_some()) {
        (String::new(), current("XDG_RUNTIME_DIR").map(PathBuf::from))
    } else { sources(&current) };
    complete(current, &systemd, runtime.as_deref())
}

/// `missing` com o ambiente deste processo; fora do Linux, nada.
pub fn fill() -> Vec<(String, String)> {
    if !cfg!(target_os = "linux") { return Vec::new(); }
    let current = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let (systemd, runtime) = sources(&current);
    missing(current, &systemd, runtime.as_deref())
}

fn sources(current: &impl Fn(&str) -> Option<String>) -> (String, Option<PathBuf>) {
    let systemd = systemd_environment().unwrap_or_default();
    let runtime = current("XDG_RUNTIME_DIR").map(PathBuf::from)
        .or_else(|| systemd.lines().find_map(|l| l.strip_prefix("XDG_RUNTIME_DIR=")).map(PathBuf::from));
    (systemd, runtime)
}

/// Com prazo: barramento do usuário quebrado não pode prender quem lança.
fn systemd_environment() -> Option<String> {
    read_with_limit(std::process::Command::new("systemctl").args(["--user", "show-environment"]), std::time::Duration::from_secs(3))
}

/// A saída é lida enquanto o filho roda (pipe cheio o prenderia); estourado o prazo, ele é morto e colhido.
fn read_with_limit(command: &mut std::process::Command, limit: std::time::Duration) -> Option<String> {
    let mut child = command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn()
        .map_err(|e| tracing::info!(target: "voice", "display env: spawn failed kind={:?}", e.kind())).ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let mut out = String::new(); let _ = tx.send(std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out)); });
    let read = rx.recv_timeout(limit);
    if read.is_err() { let _ = child.kill(); }
    let _ = child.wait();
    match read {
        Ok(Ok(out)) => Some(out),
        _ => { tracing::info!(target: "voice", "display env: user manager environment unreadable in time"); None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned())
    }

    #[test]
    fn takes_what_is_missing_from_the_user_manager_and_keeps_what_the_process_has() {
        let systemd = "PATH=/usr/bin\nWAYLAND_DISPLAY=wayland-1\nDISPLAY=:1\nHYPRLAND_INSTANCE_SIGNATURE=abc_1\nXDG_RUNTIME_DIR=/run/user/1000\n";
        let got = missing(env(&[("DISPLAY", ":0")]), systemd, None);
        assert_eq!(got, [("WAYLAND_DISPLAY".into(), "wayland-1".into()), ("HYPRLAND_INSTANCE_SIGNATURE".into(), "abc_1".into()),
            ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())], "DISPLAY do processo fica; PATH não é de tela");
        assert!(missing(env(&[("WAYLAND_DISPLAY", "w"), ("DISPLAY", ":0"), ("HYPRLAND_INSTANCE_SIGNATURE", "s"), ("XDG_RUNTIME_DIR", "/r")]),
            systemd, None).is_empty());
    }

    #[test]
    fn without_the_user_manager_the_runtime_dir_still_tells_the_wayland_socket_and_the_newest_hyprland() {
        let run = tempfile::tempdir().unwrap();
        std::fs::write(run.path().join("wayland-1.lock"), "").unwrap();
        std::fs::write(run.path().join("wayland-1"), "").unwrap();
        let hypr = run.path().join("hypr");
        std::fs::create_dir_all(hypr.join("old_1")).unwrap();
        // estavel: o que se testa é a escolha da pasta de mtime mais novo; 20 ms passa da resolução de mtime dos sistemas suportados.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(hypr.join("new_2")).unwrap();
        let got = missing(env(&[("XDG_RUNTIME_DIR", "x")]), "", Some(run.path()));
        assert_eq!(got, [("WAYLAND_DISPLAY".into(), "wayland-1".into()), ("HYPRLAND_INSTANCE_SIGNATURE".into(), "new_2".into())]);
    }

    #[test]
    fn a_stale_instance_from_the_user_manager_loses_to_the_one_that_exists() {
        let run = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(run.path().join("hypr").join("vivo_2")).unwrap();
        std::fs::write(run.path().join("wayland-1"), "").unwrap();
        let systemd = "WAYLAND_DISPLAY=wayland-0\nHYPRLAND_INSTANCE_SIGNATURE=morto_1\n";
        let got = missing(env(&[("DISPLAY", ":0"), ("XDG_RUNTIME_DIR", "x")]), systemd, Some(run.path()));
        assert_eq!(got, [("WAYLAND_DISPLAY".into(), "wayland-1".into()), ("HYPRLAND_INSTANCE_SIGNATURE".into(), "vivo_2".into())]);
        std::fs::create_dir_all(run.path().join("hypr").join("morto_1")).unwrap();
        let got = missing(env(&[("DISPLAY", ":0"), ("XDG_RUNTIME_DIR", "x"), ("WAYLAND_DISPLAY", "wayland-1")]), systemd, Some(run.path()));
        assert_eq!(got, [("HYPRLAND_INSTANCE_SIGNATURE".into(), "morto_1".into())], "existindo, o do gerenciador vale");
    }

    #[test]
    fn a_dead_value_of_the_process_is_replaced_by_the_one_that_exists() {
        let run = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(run.path().join("hypr").join("vivo_2")).unwrap();
        std::fs::write(run.path().join("wayland-1"), "").unwrap();
        let current = env(&[("HYPRLAND_INSTANCE_SIGNATURE", "morto_1"), ("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0"), ("XDG_RUNTIME_DIR", "x")]);
        assert_eq!(complete(current, "", Some(run.path())), [("WAYLAND_DISPLAY".into(), "wayland-1".into()), ("DISPLAY".into(), ":0".into()),
            ("HYPRLAND_INSTANCE_SIGNATURE".into(), "vivo_2".into()), ("XDG_RUNTIME_DIR".into(), "x".into())]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn with_every_value_present_the_dead_instance_is_still_replaced() {
        let run = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(run.path().join("hypr").join("vivo_2")).unwrap();
        std::fs::write(run.path().join("wayland-1"), "").unwrap();
        let dir = run.path().to_string_lossy().into_owned();
        let current = |key: &str| Some(match key { "HYPRLAND_INSTANCE_SIGNATURE" => "morto_1".to_owned(), "WAYLAND_DISPLAY" => "wayland-1".to_owned(),
            "DISPLAY" => ":0".to_owned(), _ => dir.clone() });
        assert_eq!(for_launch(current)[2], ("HYPRLAND_INSTANCE_SIGNATURE".into(), "vivo_2".into()));
    }

    #[test]
    fn a_large_user_environment_is_read_whole() {
        let big = "head -c 200000 /dev/zero | tr '\\0' a; echo; echo WAYLAND_DISPLAY=wayland-9";
        assert!(read_with_limit(std::process::Command::new("sh").args(["-c", big]), std::time::Duration::from_secs(5))
            .is_some_and(|out| out.contains("WAYLAND_DISPLAY=wayland-9")), "pipe cheio não pode prender o filho");
        assert!(read_with_limit(std::process::Command::new("sh").args(["-c", "sleep 5"]), std::time::Duration::from_millis(200)).is_none());
    }

    #[test]
    fn nothing_found_is_nothing_filled() {
        let run = tempfile::tempdir().unwrap();
        assert!(missing(env(&[]), "garbage without equals\n", Some(run.path())).is_empty());
    }
}
