//! Conferência prévia no app, antes do clone e sem mexer em nada (spec "Conferência prévia", etapa 1): git, curl (Linux),
//! internet até o GitHub, espaço livre no destino, winget (Windows), gerenciador de pacotes e sudo (Linux).
use std::{path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};
use super::system::{find_program, hidden, refreshed_path};

pub(crate) const GB: u64 = 1024 * 1024 * 1024;
/// Clone, venv, node_modules e agentes. ponytail: número de folga, subir quando a instalação crescer.
pub(crate) const MIN_FREE: u64 = 2 * GB;
const HOSTS: [&str; 2] = ["https://github.com", "https://raw.githubusercontent.com"];
/// Mesma ordem do `sugere_git` do `bootstrap.sh`, sem o brew (macOS fica fora).
const PKG_MANAGERS: [(&str, &[&str]); 5] = [
    ("pacman", &["pacman", "-S", "--needed", "--noconfirm", "git"]),
    ("apt-get", &["apt-get", "install", "-y", "git"]),
    ("dnf", &["dnf", "install", "-y", "git"]),
    ("zypper", &["zypper", "--non-interactive", "install", "git"]),
    ("apk", &["apk", "add", "git"]),
];

#[derive(Clone, Debug, PartialEq)]
/// `Unconfirmed`: fora dos grupos de administrador, mas o sudoers pode ter uma linha da pessoa; quem decide é o `sudo -S -v` do script.
pub(crate) enum Sudo { Ready, Unconfirmed(String), Missing }

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Facts {
    pub windows: bool,
    /// Instala o pacote: sem clone, o git não entra.
    pub package: bool,
    pub git: bool,
    pub curl: bool,
    /// Primeiro host que não respondeu e o motivo.
    pub internet: Result<(), (String, String)>,
    pub free: Result<u64, String>,
    pub dest: String,
    pub winget: bool,
    pub pkg: Option<&'static str>,
    pub sudo: Sudo,
    /// A conferência local caiu: nada acima vale, e "falta o git" seria mentira.
    pub internal: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check { Git, Curl, Internet, Space, Winget, Pkg, Sudo, Internal }

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CheckRow { pub check: Check, pub ok: bool, pub blocking: bool, pub detail: String, pub code: Option<&'static str> }

fn row(check: Check, ok: bool, detail: String, code: Option<&'static str>) -> CheckRow { CheckRow { check, ok, blocking: !ok, detail, code } }

pub(crate) fn rows(f: &Facts) -> Vec<CheckRow> {
    if let Some(why) = &f.internal { return vec![row(Check::Internal, false, why.clone(), None)]; }
    // Falta o git não trava: o app o instala antes de seguir. O pacote não usa git.
    let mut rows: Vec<CheckRow> = (!f.package).then(|| CheckRow { check: Check::Git, ok: f.git, blocking: false, detail: String::new(), code: None })
        .into_iter().collect();
    if !f.windows { rows.push(row(Check::Curl, f.curl, String::new(), None)); }
    rows.push(match &f.internet {
        Ok(()) => row(Check::Internet, true, String::new(), None),
        Err((host, why)) => row(Check::Internet, false, format!("{host}: {why}"), Some("sem-internet")),
    });
    rows.push(match &f.free {
        Ok(free) => row(Check::Space, *free >= MIN_FREE, format!("{} GB · {}", free / GB, f.dest), None),
        Err(why) => CheckRow { blocking: false, ..row(Check::Space, false, why.clone(), None) },
    });
    if f.windows {
        rows.push(row(Check::Winget, f.winget, String::new(), Some("sem-winget")));
    } else {
        rows.push(row(Check::Pkg, f.pkg.is_some(), f.pkg.unwrap_or("").to_owned(), None));
        rows.push(match &f.sudo {
            Sudo::Ready => row(Check::Sudo, true, String::new(), None),
            Sudo::Unconfirmed(why) => CheckRow { blocking: false, ..row(Check::Sudo, false, why.clone(), None) },
            Sudo::Missing => row(Check::Sudo, false, "sudo".into(), Some("sem-sudo")),
        });
    }
    rows
}

pub(crate) fn blocked(rows: &[CheckRow]) -> bool { rows.iter().any(|r| r.blocking) }
pub(crate) fn needs_git(rows: &[CheckRow]) -> bool { rows.iter().any(|r| r.check == Check::Git && !r.ok) }

/// Sem senha guardada, `sudo -n true` falha mesmo para quem pode: o grupo responde sem pedir senha.
pub(crate) fn sudo_from(n_ok: bool, groups: &str) -> Sudo {
    if n_ok || groups.split_whitespace().any(|g| matches!(g, "sudo" | "wheel" | "admin")) { Sudo::Ready }
    else { Sudo::Unconfirmed(format!("id -Gn: {}", groups.trim())) }
}

pub(crate) fn git_command(pkg: &str) -> Option<&'static [&'static str]> { PKG_MANAGERS.iter().find(|(name, _)| *name == pkg).map(|(_, cmd)| *cmd) }

pub(crate) async fn gather(dest: PathBuf, package: bool) -> Facts {
    let internet = internet().await;
    let dest_text = dest.to_string_lossy().into_owned();
    let local = tokio::task::spawn_blocking(move || {
        let path = refreshed_path();
        let has = |name: &str| find_program(name, &path).is_some();
        (has("git"), has("curl"), has("winget"), PKG_MANAGERS.iter().map(|(name, _)| *name).find(|name| has(name)),
            free_space(&dest), if cfg!(windows) { Sudo::Missing } else { sudo(&path) })
    }).await;
    let internal = local.as_ref().err().map(|e| e.to_string());
    let (git, curl, winget, pkg, free, sudo) = local.unwrap_or_else(|e| (false, false, false, None, Err(e.to_string()), Sudo::Missing));
    Facts { windows: cfg!(windows), package, git, curl, internet, free, dest: dest_text, winget, pkg, sudo, internal }
}

async fn internet() -> Result<(), (String, String)> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| (String::new(), e.to_string()))?;
    for host in HOSTS {
        // Qualquer resposta HTTP prova a rota; só a falha de conexão conta.
        if let Err(error) = client.head(host).send().await { return Err((host.trim_start_matches("https://").to_owned(), error.to_string())); }
    }
    Ok(())
}

fn sudo(path: &str) -> Sudo {
    let Some(sudo) = find_program("sudo", path) else { return Sudo::Missing };
    let cached = hidden(&mut Command::new(sudo)).args(["-n", "true"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status().is_ok_and(|s| s.success());
    let groups = hidden(&mut Command::new("id")).arg("-Gn").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    sudo_from(cached, &groups)
}

/// O destino pode ainda não existir: mede no primeiro pai que existe.
#[cfg(target_os = "linux")]
fn free_space(dest: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let probe = dest.ancestors().find(|p| p.exists()).ok_or_else(|| dest.display().to_string())?;
    let c_path = std::ffi::CString::new(probe.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 { return Err(std::io::Error::last_os_error().to_string()); }
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
fn free_space(dest: &Path) -> Result<u64, String> {
    let dest = dest.to_string_lossy().replace('\'', "''");
    super::system::powershell(&format!("[IO.DriveInfo]::new([IO.Path]::GetPathRoot('{dest}')).AvailableFreeSpace"))?
        .trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn free_space(_: &Path) -> Result<u64, String> { Err("unsupported".into()) }

/// Linux: a senha da janela autentica o sudo à parte (`-v`, stdin), e o comando roda com `sudo -n` e stdin nulo: com regra
/// NOPASSWD a senha nunca viraria entrada do comando. Os dois são filhos do app, então o cache do sudo vale. Windows: winget.
pub(crate) fn install_git(pkg: Option<&'static str>, password: Option<&str>) -> Result<(), String> {
    use std::io::Write;
    let path = refreshed_path();
    let output = if cfg!(windows) {
        let winget = find_program("winget", &path).ok_or("winget")?;
        hidden(&mut Command::new(winget)).args(["install", "--id", "Git.Git", "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements"])
            .env("PATH", &path).stdin(Stdio::null()).output().map_err(|e| e.to_string())?
    } else {
        let command = pkg.and_then(git_command).ok_or("pkg")?;
        let sudo = find_program("sudo", &path).ok_or("sudo")?;
        if let Some(password) = password {
            let mut child = hidden(&mut Command::new(&sudo)).args(["-S", "-p", "", "-v"]).env("PATH", &path)
                .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
            if let Some(mut stdin) = child.stdin.take() { let _ = writeln!(stdin, "{password}"); }
            let auth = child.wait_with_output().map_err(|e| e.to_string())?;
            if !auth.status.success() { return Err(String::from_utf8_lossy(&auth.stderr).trim().to_owned()); }
        }
        hidden(&mut Command::new(&sudo)).arg("-n").args(command).env("PATH", &path)
            .stdin(Stdio::null()).output().map_err(|e| e.to_string())?
    };
    if find_program("git", &refreshed_path()).is_some() { return Ok(()); }
    let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if text.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_owned() } else { text })
}

/// `pacotes-desatualizados`: o script só dá esse código no apt (`Unable to locate package`), então só o apt é atualizado.
/// Mesmo jeito do `install_git`: a senha autentica o sudo à parte e o comando roda com `sudo -n` e stdin nulo.
pub(crate) fn refresh_package_list(password: &str) -> Result<(), String> {
    use std::io::Write;
    let path = refreshed_path();
    let sudo = find_program("sudo", &path).ok_or("sudo")?;
    let mut child = hidden(&mut Command::new(&sudo)).args(["-S", "-p", "", "-v"]).env("PATH", &path)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() { let _ = writeln!(stdin, "{password}"); }
    let auth = child.wait_with_output().map_err(|e| e.to_string())?;
    if !auth.status.success() { return Err(String::from_utf8_lossy(&auth.stderr).trim().to_owned()); }
    let out = hidden(&mut Command::new(&sudo)).args(["-n", "apt-get", "update"]).env("PATH", &path)
        .stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() { return Ok(()); }
    let text = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(if text.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_owned() } else { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> Facts {
        Facts { windows: false, package: false, git: true, curl: true, internet: Ok(()), free: Ok(10 * GB), dest: "/home/dev/hangar".into(),
            winget: false, pkg: Some("apt-get"), sudo: Sudo::Ready, internal: None }
    }

    #[test]
    fn crashed_local_check_is_an_internal_error_not_a_missing_git() {
        let rows = rows(&Facts { git: false, internal: Some("task panicked".into()), ..linux() });
        assert_eq!(rows.iter().map(|r| (r.check, r.blocking)).collect::<Vec<_>>(), vec![(Check::Internal, true)]);
        assert!(!needs_git(&rows));
    }

    #[test]
    fn ready_linux_is_not_blocked() {
        let rows = rows(&linux());
        assert_eq!(rows.iter().map(|r| r.check).collect::<Vec<_>>(),
            vec![Check::Git, Check::Curl, Check::Internet, Check::Space, Check::Pkg, Check::Sudo]);
        assert!(!blocked(&rows));
        assert!(!needs_git(&rows));
    }

    #[test]
    fn missing_git_is_installed_not_blocking() {
        let rows = rows(&Facts { git: false, ..linux() });
        assert!(needs_git(&rows));
        assert!(!blocked(&rows));
    }

    #[test]
    fn package_install_skips_git() {
        for windows in [false, true] {
            let rows = rows(&Facts { package: true, git: false, windows, ..linux() });
            assert!(!rows.iter().any(|r| r.check == Check::Git), "windows={windows}");
            assert!(!needs_git(&rows));
        }
    }

    #[test]
    fn each_missing_piece_blocks_with_its_code() {
        let code = |f: Facts, check: Check| rows(&f).into_iter().find(|r| r.check == check).map(|r| (r.blocking, r.code));
        assert_eq!(code(Facts { internet: Err(("github.com".into(), "dns".into())), ..linux() }, Check::Internet), Some((true, Some("sem-internet"))));
        assert_eq!(code(Facts { sudo: Sudo::Missing, ..linux() }, Check::Sudo), Some((true, Some("sem-sudo"))));
        assert_eq!(code(Facts { curl: false, ..linux() }, Check::Curl), Some((true, None)));
        assert_eq!(code(Facts { pkg: None, ..linux() }, Check::Pkg), Some((true, None)));
        assert_eq!(code(Facts { free: Ok(MIN_FREE - 1), ..linux() }, Check::Space), Some((true, None)));
        // Espaço que não deu para medir aparece, mas não trava.
        assert_eq!(code(Facts { free: Err("statvfs".into()), ..linux() }, Check::Space), Some((false, None)));
    }

    #[test]
    fn windows_checks_winget_and_skips_linux_tools() {
        let f = Facts { windows: true, curl: false, pkg: None, sudo: Sudo::Missing, winget: false, ..linux() };
        let rows = rows(&f);
        assert_eq!(rows.iter().map(|r| r.check).collect::<Vec<_>>(), vec![Check::Git, Check::Internet, Check::Space, Check::Winget]);
        assert_eq!(rows.last().map(|r| (r.blocking, r.code)), Some((true, Some("sem-winget"))));
    }

    #[test]
    fn sudo_by_cached_password_or_admin_group() {
        assert_eq!(sudo_from(true, ""), Sudo::Ready);
        assert_eq!(sudo_from(false, "dev wheel audio"), Sudo::Ready);
        assert!(matches!(sudo_from(false, "dev audio"), Sudo::Unconfirmed(_)));
    }

    #[test]
    fn sudo_outside_admin_groups_warns_without_blocking() {
        let rows = rows(&Facts { sudo: sudo_from(false, "dev audio"), ..linux() });
        let sudo = rows.iter().find(|r| r.check == Check::Sudo).unwrap();
        assert_eq!((sudo.ok, sudo.blocking, sudo.code), (false, false, None));
        assert!(!blocked(&rows));
    }

    #[test]
    fn git_comes_from_the_package_manager_found() {
        assert_eq!(git_command("apt-get"), Some(&["apt-get", "install", "-y", "git"][..]));
        assert_eq!(git_command("brew"), None);
    }
}
