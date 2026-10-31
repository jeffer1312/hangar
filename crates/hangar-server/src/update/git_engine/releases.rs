//! Binários do checkout, conferidos antes de publicar e com cópia para a volta.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
pub fn digest(data: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn nonce() -> std::io::Result<String> {
    let mut bytes = [0; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| std::io::Error::other("aleatoriedade indisponível"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub const BASE: &str = "https://github.com/jeffer1312/hangar/releases/download";
pub const NAMES: [&str; 3] = ["hangar-server", "hangar-cano", "hangar-auth"];
#[derive(Debug)]
pub struct DownloadError {
    pub kind: &'static str,
    pub status: Option<u16>,
    pub message: String,
}
pub trait Source {
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, DownloadError>;
    fn get_release(&self, url: &str, deadline: Duration) -> Result<Vec<u8>, DownloadError> { self.get(url, deadline) }
}
pub struct HttpSource;
impl HttpSource {
    fn fetch(&self, url: &str, timeout: Duration, socket: Option<Duration>) -> Result<Vec<u8>, DownloadError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| DownloadError { kind: "OSError",
                status: None,
                message: e.to_string(),
            })?;
        runtime.block_on(async {
            let mut builder = reqwest::Client::builder().timeout(timeout);
            if let Some(socket) = socket {
                builder = builder.connect_timeout(socket).read_timeout(socket);
            }
            let client = builder.build()
                .map_err(|e| DownloadError { kind: "ValueError",
                    status: None,
                    message: e.to_string(),
                })?;
            let response = client.get(url).send().await.map_err(|e| DownloadError { kind: if e.is_timeout() && !e.is_connect() {"TimeoutError"} else {"URLError"},
                status: None,
                message: e.to_string(),
            })?;
            let status = response.status();
            if !status.is_success() {
                return Err(DownloadError { kind: "HTTPError",
                    status: Some(status.as_u16()),
                    message: format!("HTTP {}", status.as_u16()),
                });
            }
            response
                .bytes()
                .await
                .map(|v| v.to_vec())
                .map_err(|e| DownloadError { kind: if e.is_timeout(){"TimeoutError"}else{"IncompleteRead"},
                    status: None,
                    message: e.to_string(),
                })
        })
    }
}
impl Source for HttpSource {
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, DownloadError> {
        self.fetch(url, timeout, None)
    }
    fn get_release(&self, url: &str, deadline: Duration) -> Result<Vec<u8>, DownloadError> {
        self.fetch(url, deadline, Some(Duration::from_secs(15)))
    }
}
#[derive(Clone)]
pub struct Release {
    pub url: String,
    pub tag: Option<String>,
    pub manifest: Value,
}
pub struct Held {
    pub data: Vec<u8>,
    pub target: PathBuf,
    pub name: String,
}
pub fn tag_for(branch: &str) -> String {
    if matches!(branch, "main" | "master" | "HEAD") {
        "server-latest".into()
    } else {
        format!(
            "server-{}",
            branch
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '-'
                })
                .collect::<String>()
        )
    }
}
pub fn read(source: &dyn Source, url: &str) -> Result<Value, DownloadError> {
    let data = source.get_release(
        &format!("{}/server-latest.json", url.trim_end_matches('/')),
        Duration::from_secs(30),
    )?;
    let value: Value = serde_json::from_slice(&data).map_err(|e| DownloadError { kind: "JSONDecodeError",
        status: None,
        message: e.to_string(),
    })?;
    if !value.is_object() {
        return Err(DownloadError { kind: "ValueError",
            status: None,
            message: "manifesto não é um objeto".into(),
        });
    }
    Ok(value)
}
pub fn has_build(manifest: &Value, platform: &str) -> bool {
    NAMES
        .iter()
        .all(|n| manifest["files"][format!("{platform}/{n}")].is_object())
}
pub fn matches_file(path: &Path, sha: &str) -> bool {
    fs::read(path).is_ok_and(|data| digest(&data) == sha)
}
fn previous(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.prev",
        path.file_name().unwrap().to_string_lossy()
    ))
}
fn targets(dest: &Path, platform: &str) -> Vec<(&'static str, PathBuf)> {
    let ext = if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    };
    NAMES
        .into_iter()
        .filter(|name| *name != "hangar-auth")
        .map(|name| (name, dest.join(format!("{name}{ext}"))))
        .chain(std::iter::once((
            "hangar-auth-pair",
            dest.join("hangar-server.auth.json"),
        )))
        .collect()
}
fn sweep(target: &Path) {
    let prefix = format!("{}.old", target.file_name().unwrap().to_string_lossy()).to_lowercase();
    if let Ok(entries) = fs::read_dir(target.parent().unwrap()) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&prefix)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}
pub fn install(data: &[u8], target: &Path) -> std::io::Result<()> {
    let tmp = target.with_file_name(format!(
        ".{}.{}.tmp",
        target.file_name().unwrap().to_string_lossy(),
        nonce()?
    ));
    let result = (|| {
        fs::write(&tmp, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        }
        #[cfg(windows)]
        if target.extension().and_then(|v| v.to_str()) != Some("json")
            && fs::symlink_metadata(target).is_ok()
        {
            let name = target.file_name().unwrap().to_string_lossy();
            let old=(0..100).map(|n|target.with_file_name(if n==0{format!("{name}.old")}else{format!("{name}.old-{n}")})).find(|p|matches!(fs::symlink_metadata(p),Err(e) if e.kind()==std::io::ErrorKind::NotFound)).ok_or_else(||std::io::Error::other(format!("sem nome livre para {name}.old")))?;
            fs::rename(target, &old)?;
            if let Err(e) = fs::rename(&tmp, target) {
                let _ = fs::rename(&old, target);
                return Err(e);
            }
            return Ok(());
        }
        fs::rename(&tmp, target)
    })();
    let _ = fs::remove_file(tmp);
    result
}
fn install_warning(data: &[u8], target: &Path, name: &str) -> Option<String> {
    install(data, target).err().map(|e| {
        format!(
            "{name}: não consegui gravar em {} ({e})",
            target.parent().unwrap().display()
        )
    })
}
pub fn publish(held: &[Held]) -> Vec<String> {
    super::auth_pair::publish(held, true)
}
pub fn restore(dest: &Path, platform: &str) -> Vec<String> {
    let mut warnings = vec![];
    for (name, target) in targets(dest, platform)
        .into_iter()
        .filter(|(name, _)| *name == "hangar-cano")
    {
        let prev = previous(&target);
        if !prev.is_file() {
            continue;
        }
        if let Err(e) = fs::read(&prev)
            .and_then(|data| install(&data, &target))
            .and_then(|_| fs::remove_file(prev))
        {
            warnings.push(format!(
                "{name}: não consegui devolver o binário anterior ({e})"
            ));
        }
    }
    let server = dest.join(if platform.starts_with("windows") {
        "hangar-server.exe"
    } else {
        "hangar-server"
    });
    warnings.extend(super::auth_pair::restore(&server));
    warnings
}
pub fn discard(dest: &Path, platform: &str) -> Vec<String> {
    let mut warnings = vec![];
    for (name, target) in targets(dest, platform) {
        if let Err(e) = fs::remove_file(previous(&target)) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warnings.push(format!(
                    "{name}: a cópia anterior do binário não saiu de {} ({e})",
                    dest.display()
                ));
            }
        }
    }
    warnings
}
pub fn fetch(
    source: &dyn Source,
    base: &str,
    tag: Option<&str>,
    platform: Option<&str>,
    dest: &Path,
    release: Option<&Release>,
    mut hold: Option<&mut Vec<Held>>,
) -> Option<Vec<String>> {
    let platform = platform?;
    let mut url = release.map(|r| r.url.clone()).unwrap_or_else(|| {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            tag.unwrap_or("server-latest")
        )
    });
    let mut current_tag = release
        .and_then(|r| r.tag.clone())
        .or_else(|| tag.map(str::to_owned));
    let mut branch = None;
    let mut missing = "ausente".to_string();
    let manifest = (|| {
        let first = release
            .map(|r| Ok(r.manifest.clone()))
            .unwrap_or_else(|| read(source, &url));
        let value = match first {
            Ok(value)
                if current_tag.as_deref().is_some_and(|t| t != "server-latest")
                    && !has_build(&value, platform) =>
            {
                missing = format!("sem build para {platform}");
                None
            }
            Ok(value) => Some(value),
            Err(e)
                if e.status == Some(404)
                    && current_tag.as_deref().is_some_and(|t| t != "server-latest") =>
            {
                None
            }
            Err(e) => return Err(e.message),
        };
        let value = if let Some(value) = value {
            value
        } else {
            branch = current_tag.take();
            url = format!("{}/server-latest", base.trim_end_matches('/'));
            read(source, &url).map_err(|e| e.message)?
        };
        if value.get("files").is_none() {
            return Err("'files'".into());
        }
        fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        Ok(value)
    })();
    let manifest = match manifest {
        Ok(value) => value,
        Err(e) => {
            let via = branch
                .as_ref()
                .map(|tag| format!(" da main (release {tag} {missing})"))
                .unwrap_or_default();
            return Some(vec![format!(
                "binários Rust não baixados: não consegui ler o manifesto da release{via} ({e})"
            )]);
        }
    };
    let mut warnings = vec![];
    let mut kept = vec![];
    let mut installed = vec![];
    let mut pending = Vec::new();
    let mut pair_failed = false;
    if !has_build(&manifest, platform) {
        let prefix = branch
            .as_ref()
            .map(|tag| format!("release {tag} {missing}; da main, "))
            .unwrap_or_default();
        return Some(
            NAMES
                .iter()
                .filter(|name| !manifest["files"][format!("{platform}/{name}")].is_object())
                .map(|name| format!("{prefix}{name}: a release não traz build para {platform}"))
                .collect(),
        );
    }
    let (library, metadata) = match super::auth_pair::metadata(&manifest, platform, dest) {
        Ok(v) => v,
        Err(e) => return Some(vec![e]),
    };
    let ext = if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    };
    for name in ["hangar-auth", "hangar-cano", "hangar-server"] {
        let target = if name == "hangar-auth" {
            library.clone()
        } else {
            dest.join(format!("{name}{ext}"))
        };
        if name != "hangar-auth" {
            sweep(&target);
        }
        if branch.is_some()
            && (fs::symlink_metadata(&target).is_ok()
                || (name == "hangar-auth"
                    && fs::symlink_metadata(dest.join(format!("hangar-server{ext}"))).is_ok()))
        {
            kept.push(name);
            continue;
        }
        let entry = &manifest["files"][format!("{platform}/{name}")];
        let result = (|| {
            let (Some(asset), Some(sha)) = (entry["name"].as_str(), entry["sha256"].as_str())
            else {
                return Some(format!("{name}: a release não traz build para {platform}"));
            };
            let sha = sha.to_lowercase();
            if matches_file(&target, &sha) {
                return None;
            }
            let data = match source.get_release(&format!("{url}/{asset}"), Duration::from_secs(200)) {
                Ok(data) => data,
                Err(e) => return Some(format!("{name}: o download falhou ({})", e.message)),
            };
            if digest(&data) != sha {
                return Some(format!(
                    "{name}: o sha256 não confere com a release; o binário anterior ficou"
                ));
            }
            if hold.is_some() || name != "hangar-cano" {
                pending.push(Held {
                    data,
                    target: target.clone(),
                    name: name.into(),
                });
                None
            } else {
                install_warning(&data, &target, name)
            }
        })();
        if let Some(warning) = result {
            pair_failed |= name != "hangar-cano";
            warnings.push(if let Some(branch) = &branch {
                format!("release {branch} {missing}; da main, {warning}")
            } else {
                warning
            });
        } else if branch.is_some() {
            installed.push(name);
        }
    }
    if !pair_failed {
        if !kept.contains(&"hangar-server") {
            let target = dest.join("hangar-server.auth.json");
            if !matches_file(&target, &digest(&metadata)) {
                pending.push(Held {
                    data: metadata,
                    target,
                    name: "hangar-auth-pair".into(),
                });
            }
        }
        if let Some(held) = hold.as_deref_mut() {
            held.extend(pending);
        } else {
            warnings.extend(super::auth_pair::publish(&pending, false));
        }
    } else if let Some(held) = hold.as_deref_mut() {
        held.extend(
            pending
                .into_iter()
                .filter(|item| item.name == "hangar-cano"),
        );
    }
    if hold.is_none() {
        installed.retain(|name| {
            let target = if *name == "hangar-auth" {
                library.clone()
            } else {
                dest.join(format!("{name}{ext}"))
            };
            manifest["files"][format!("{platform}/{name}")]["sha256"]
                .as_str()
                .is_some_and(|sha| matches_file(&target, &sha.to_lowercase()))
        });
    }
    if let Some(branch) = branch {
        if !kept.is_empty() {
            warnings.push(format!(
                "release {branch} {missing}; mantive os binários instalados ({})",
                kept.join(", ")
            ));
        }
        if !installed.is_empty() {
            warnings.push(format!(
                "release {branch} {missing}; instalei os da main ({})",
                installed.join(", ")
            ));
        }
    }
    Some(warnings)
}
/// Download direto para o primeiro binário do checkout, sem importar módulos Python.
pub fn cli(args: &[String]) -> std::process::ExitCode {
    let Some(checkout) = args.first().map(PathBuf::from) else {
        return std::process::ExitCode::from(2);
    };
    if args.len() != 1 {
        return std::process::ExitCode::from(2);
    }
    let environment = crate::accounts::environment::AccountEnvironment::capture();
    let dest = environment.home.join(".hangar/bin");
    let platform = crate::update::platform();
    if !matches!(
        platform.as_str(),
        "linux-x86_64" | "windows-x86_64" | "macos-x86_64" | "macos-aarch64"
    ) {
        println!(
            "binários Rust: a release não tem build para esta máquina; o Python atende sozinho"
        );
        return std::process::ExitCode::from(2);
    }
    let branch = crate::update::process::run(
        &[
            "git".into(),
            "rev-parse".into(),
            "--abbrev-ref".into(),
            "HEAD".into(),
        ],
        &checkout,
        Duration::from_secs(5),
        &|_| {},
    )
    .ok()
    .filter(|output| output.code == 0 && !output.stdout.trim().is_empty())
    .map(|output| output.stdout.trim().to_owned())
    .unwrap_or_else(|| "main".into());
    let tag = tag_for(&branch);
    let source = HttpSource;
    let explicit = environment
        .base
        .get("HANGAR_SERVER_RELEASE_URL")
        .filter(|value| !value.is_empty());
    let release = if let Some(url) = explicit {
        match read(&source, url) {
            Ok(manifest) => Some(Release {
                url: url.trim_end_matches('/').into(),
                tag: None,
                manifest,
            }),
            Err(error) => {
                println!(
                    "binários Rust não baixados: não consegui ler o manifesto da release ({})",
                    error.message
                );
                return std::process::ExitCode::from(1);
            }
        }
    } else {
        None
    };
    let result = fetch(
        &source,
        BASE,
        if explicit.is_some() { None } else { Some(&tag) },
        Some(&platform),
        &dest,
        release.as_ref(),
        None,
    );
    match result {
        None => std::process::ExitCode::from(2),
        Some(warnings) if warnings.is_empty() => {
            println!("binários Rust em {}", dest.display());
            std::process::ExitCode::SUCCESS
        }
        Some(warnings) => {
            for warning in warnings {
                println!("{warning}");
            }
            std::process::ExitCode::from(1)
        }
    }
}
