//! Onde está o CLIProxyAPI desta máquina: instalado pelo Hangar, por outro caminho, ou em lugar nenhum.
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `~/.hangar/cliproxy`: programa por versão e o `install.json`.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub dir: PathBuf,
}

impl Paths {
    pub fn new(home: &Path) -> Self {
        Self { home: home.to_owned(), dir: home.join(".hangar").join("cliproxy") }
    }

    pub fn record(&self) -> PathBuf {
        self.dir.join("install.json")
    }

    pub fn config(&self) -> PathBuf {
        crate::config::path(&self.home)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Absent,
    /// Pacote do sistema, binário no PATH ou serviço que o Hangar não criou: nunca é trocado.
    External { binary: Option<PathBuf>, unit: Option<PathBuf> },
    Managed { tag: String, binary: PathBuf },
}

/// Units que pacotes do proxy instalam (o AUR `cli-proxy-api-bin` usa a de usuário em `/usr/lib`); a de sistema
/// escrita à mão fica em `/etc/systemd/system`. Na ordem em que o systemd escolhe: a cópia em `~/.config` ou `/etc`
/// vence a do pacote, e o primeiro arquivo que existe é o que roda.
pub fn system_units(home: &Path) -> Vec<PathBuf> {
    std::iter::once(home.join(".config/systemd/user/cli-proxy-api.service"))
        .chain(["/etc/systemd/user/cli-proxy-api.service", "/usr/lib/systemd/user/cli-proxy-api.service",
            "/etc/systemd/system/cli-proxy-api.service", "/usr/lib/systemd/system/cli-proxy-api.service"].into_iter().map(PathBuf::from))
        .collect()
}

/// Unit de sistema: o `%h` dela é a pasta do `User=` (ou do root), não a desta conta.
pub fn is_system_unit(unit: &Path) -> bool {
    unit.starts_with("/etc/systemd/system") || unit.starts_with("/usr/lib/systemd/system")
}

/// Algum `<unit>.d/*.conf` troca o `ExecStart`: o comando que roda não é o do arquivo da unit.
pub fn exec_overridden(units: &[PathBuf]) -> bool {
    units.iter().filter_map(|unit| std::fs::read_dir(format!("{}.d", unit.display())).ok()).flatten().flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "conf"))
        .any(|entry| std::fs::read_to_string(entry.path()).is_ok_and(|text| text.lines().any(|l| l.trim_start().starts_with("ExecStart="))))
}

/// O `--config` (ou `-config`) que a unit passa ao proxy: outro arquivo que não o lido pelo Hangar.
pub fn unit_config(unit: &str) -> Option<PathBuf> {
    let exec = unit.lines().map(str::trim).find_map(|l| l.strip_prefix("ExecStart="))?;
    let words: Vec<&str> = exec.split_whitespace().map(|w| w.trim_matches(['"', '\''])).collect();
    words.iter().enumerate().find_map(|(i, w)| {
        let flag = w.trim_start_matches('-');
        match flag.split_once('=') {
            Some(("config", path)) if w.starts_with('-') => Some(PathBuf::from(path.trim_matches(['"', '\'']))),
            _ if flag == "config" && w.starts_with('-') => words.get(i + 1).map(PathBuf::from),
            _ => None,
        }
    })
}

pub fn state(paths: &Paths, path_var: Option<&std::ffi::OsStr>, units: &[PathBuf]) -> Install {
    if let Some((tag, binary)) = managed(paths) {
        return Install::Managed { tag, binary };
    }
    let binary = path_var.and_then(|var| on_path(var, crate::release::Platform::local().binary()));
    let unit = units.iter().find(|p| p.is_file()).cloned();
    if binary.is_some() || unit.is_some() {
        return Install::External { binary, unit };
    }
    Install::Absent
}

fn managed(paths: &Paths) -> Option<(String, PathBuf)> {
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.record()).ok()?).ok()?;
    let tag = record["tag"].as_str().filter(|t| !t.is_empty())?.to_owned();
    let binary = PathBuf::from(record["binary"].as_str()?);
    binary.is_file().then_some((tag, binary))
}

fn on_path(var: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(var).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// Algo atende na porta: o proxy ligado, ou outro programa no lugar dele.
pub fn port_open(base_url: &str) -> bool {
    let Some(rest) = base_url.split_once("://").map(|(_, r)| r) else { return false };
    let host_port = rest.split('/').next().unwrap_or("");
    let Ok(mut addrs) = std::net::ToSocketAddrs::to_socket_addrs(host_port) else { return false };
    addrs.any(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok())
}

pub fn write_record(paths: &Paths, tag: &str, binary: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(&paths.dir)?;
    let body = serde_json::json!({ "tag": tag, "binary": binary.to_string_lossy() });
    hangar_core::fs::atomic_write(&paths.record(), body.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_wins_and_needs_the_program_on_disk() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        assert_eq!(state(&paths, None, &[]), Install::Absent);
        let binary = paths.dir.join("bin/8.0.79/cli-proxy-api");
        write_record(&paths, "v8.0.79", &binary).unwrap();
        assert_eq!(state(&paths, None, &[]), Install::Absent, "registro sem o programa não é instalação");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        let unit = home.path().join("cli-proxy-api.service");
        std::fs::write(&unit, b"[Unit]").unwrap();
        assert_eq!(state(&paths, None, &[unit]), Install::Managed { tag: "v8.0.79".into(), binary });
    }

    #[test]
    fn a_package_unit_is_external() {
        let home = tempfile::tempdir().unwrap();
        let unit = home.path().join("cli-proxy-api.service");
        std::fs::write(&unit, b"[Unit]").unwrap();
        let missing = home.path().join("nao-existe.service");
        assert_eq!(state(&Paths::new(home.path()), None, &[missing.clone(), unit.clone()]), Install::External { binary: None, unit: Some(unit) });
        assert_eq!(state(&Paths::new(home.path()), None, &[missing]), Install::Absent);
    }

    #[test]
    fn units_follow_systemd_precedence() {
        let home = tempfile::tempdir().unwrap();
        let units = system_units(home.path());
        let at = |p: &str| units.iter().position(|u| u == &PathBuf::from(p)).unwrap();
        assert_eq!(units[0], home.path().join(".config/systemd/user/cli-proxy-api.service"), "a cópia do usuário vence o pacote");
        assert!(at("/etc/systemd/user/cli-proxy-api.service") < at("/usr/lib/systemd/user/cli-proxy-api.service"));
        assert!(at("/etc/systemd/system/cli-proxy-api.service") < at("/usr/lib/systemd/system/cli-proxy-api.service"));
        assert!(is_system_unit(Path::new("/etc/systemd/system/cli-proxy-api.service")));
        assert!(!is_system_unit(Path::new("/usr/lib/systemd/user/cli-proxy-api.service")));
    }

    #[test]
    fn a_drop_in_that_replaces_exec_start_is_seen() {
        let home = tempfile::tempdir().unwrap();
        let unit = home.path().join("cli-proxy-api.service");
        std::fs::write(&unit, "[Service]\nExecStart=/usr/bin/cli-proxy-api\n").unwrap();
        assert!(!exec_overridden(std::slice::from_ref(&unit)));
        let drop_in = home.path().join("cli-proxy-api.service.d");
        std::fs::create_dir_all(&drop_in).unwrap();
        std::fs::write(drop_in.join("env.conf"), "[Service]\nEnvironment=A=1\n").unwrap();
        assert!(!exec_overridden(std::slice::from_ref(&unit)), "drop-in sem ExecStart não muda o comando");
        std::fs::write(drop_in.join("override.conf"), "[Service]\nExecStart=\nExecStart=/usr/bin/cli-proxy-api --config /srv/c.yaml\n").unwrap();
        assert!(exec_overridden(&[unit]));
    }

    #[test]
    fn the_config_the_unit_passes_is_read() {
        let unit = |exec: &str| format!("[Service]\nExecStart={exec}\n");
        assert_eq!(unit_config(&unit("/usr/bin/cli-proxy-api --config /etc/cliproxy/config.yaml")), Some(PathBuf::from("/etc/cliproxy/config.yaml")));
        assert_eq!(unit_config(&unit("/usr/bin/cli-proxy-api -config=\"/srv/c.yaml\"")), Some(PathBuf::from("/srv/c.yaml")));
        assert_eq!(unit_config(&unit("/usr/bin/cli-proxy-api")), None);
        assert_eq!(unit_config("[Unit]\nDescription=x\n"), None);
    }

    #[test]
    fn a_program_on_the_path_is_external() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("usr-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join(crate::release::Platform::local().binary());
        std::fs::write(&program, b"x").unwrap();
        let var = std::env::join_paths([bin]).unwrap();
        assert!(matches!(state(&Paths::new(home.path()), Some(&var), &[]), Install::External { binary: Some(p), .. } if p == program));
    }

    #[test]
    fn port_probe_sees_a_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_open(&format!("http://127.0.0.1:{port}")));
        drop(listener);
        assert!(!port_open(&format!("http://127.0.0.1:{port}")));
    }
}
