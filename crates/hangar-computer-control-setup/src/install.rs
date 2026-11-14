//! Instala e atualiza o hcu pela release do GitHub, sem pedido. Tudo é baixado antes de gravar; o
//! `install.json` vai por último, e falha no meio deixa a instalação de antes inteira. Falha vai ao
//! diário com código; a da instalação também fica em disco para o `GET` mostrar o bloqueio.
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::{NoExpand, Regex};
use serde_json::{Map, Value, json};

use hangar_core::pycompat::pyjson;
use hangar_update_news::Kind;

use crate::{Error, Paths, files, targets};

pub const REPO: &str = "jeffer1312/hangar-computer-control";
const LINUX_ASSET: &str = "hangar-computer-control-linux-x86_64";
const EXE_ASSET: &str = "windows-agent.exe";
const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
const ASSET_TIMEOUT: Duration = Duration::from_secs(120);
const RELEASE_FAILED: &str = "computer_control_release_failed";
const NO_BINARY: &str = "computer_control_release_sem_binario";
const DISK_FAILED: &str = "computer_control_install_io_failed";
const CLIENT_FAILED: &str = "computer_control_http_failed";
const NEWS_FAILED: &str = "computer_control_news_failed";
const TAG_INCOMPARABLE: &str = "computer_control_tag_incomparavel";
const TAG_INVALID: &str = "computer_control_tag_invalida";
const ASSET_INVALID: &str = "computer_control_asset_invalido";
/// Teto de cada pedido ao GitHub: release e assets.
const MAX_BODY: u64 = 256 << 20;

// A tag vai para a URL e para o id da novidade.
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\Av?[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\z").expect("regex fixa"));
// O `\s` do Python cobre também `\x1c-\x1f`.
static WRAPPED_AGENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"exec[\s\x1c-\x1f]+[^\s\x1c-\x1f]*python[\d.]*[\s\x1c-\x1f]+[^\s\x1c-\x1f]*linux_agent\.py").expect("regex fixa")
});
static PYTHON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\Apython(?:\d+(?:\.\d+)?)?(?:\.exe)?\z").expect("regex fixa"));

/// Onde o GitHub responde; o teste aponta os dois para um servidor local.
#[derive(Clone, Debug)]
pub struct Github {
    pub api: String,
    pub download: String,
}

/// Os portões da atualização do Hangar que valem para o hcu: automações e canal do modo pacote
/// (`None` fora dele).
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub automations: bool,
    pub package_channel: Option<String>,
}

/// Se o binário do pacote mudou: quem registra o MCP refaz o registro.
#[derive(Debug, PartialEq, Eq)]
pub enum Swapped {
    No,
    Yes { tag: String },
}

#[derive(Debug, PartialEq)]
struct Release {
    tag: String,
    body: String,
    assets: Vec<String>,
}

struct Done {
    tag: String,
    body: String,
    before: Option<String>,
    skipped: Vec<String>,
}

impl Default for Github {
    fn default() -> Self {
        Self { api: "https://api.github.com".into(), download: "https://github.com".into() }
    }
}

/// Na subida e no começo de cada tique, em qualquer modo e canal: limpa as sobras de trocas
/// anteriores e instala quando falta um arquivo que a plataforma usa.
///
/// # Errors
/// Rede, GitHub, release sem binário ou disco; já gravada no diário e em `install-error.json`.
pub async fn install_if_missing(paths: &Paths, http: &reqwest::Client, github: &Github, diary: Option<&Path>) -> Result<Swapped, Error> {
    if !crate::supported() {
        return Ok(Swapped::No);
    }
    let present = {
        let (paths, diary) = (paths.clone(), diary.map(Path::to_owned));
        off_thread(move || {
            tidy(&paths);
            let present = required(&paths, cfg!(windows)).iter().all(|(path, ..)| path.is_file());
            if present && paths.migration_pending_file().exists() {
                report_skipped(diary.as_deref(), &migrate_targets(&paths));
            }
            present
        })
        .await?
    };
    if present {
        return Ok(Swapped::No);
    }
    let outcome = latest(paths, http, github, false).await;
    let (paths, diary) = (paths.clone(), diary.map(Path::to_owned));
    off_thread(move || match outcome {
        Ok(Some(done)) => {
            forget_failure(&paths);
            report_skipped(diary.as_deref(), &done.skipped);
            Ok(Swapped::Yes { tag: done.tag })
        }
        Ok(None) => Ok(Swapped::No),
        Err(error) => {
            remember_failure(&paths, &error);
            report(diary.as_deref(), "computer_control.instalar", &error);
            Err(error)
        }
    })
    .await?
}

/// O tique de hora em hora: com automações ligadas, modo pacote e canal `latest`, troca o binário
/// quando a release é mais nova que a do `install.json`, mesmo com processo do hcu vivo.
///
/// # Errors
/// Rede, GitHub, release sem binário ou disco; já gravada no diário.
pub async fn update_tick(paths: &Paths, http: &reqwest::Client, github: &Github, policy: &Policy, diary: Option<&Path>) -> Result<Swapped, Error> {
    if !crate::supported() || !policy.automations || policy.package_channel.as_deref() != Some("latest") {
        return Ok(Swapped::No);
    }
    let outcome = latest(paths, http, github, true).await;
    let (paths, diary) = (paths.clone(), diary.map(Path::to_owned));
    off_thread(move || match outcome {
        Ok(Some(done)) => {
            report_skipped(diary.as_deref(), &done.skipped);
            let item = news_item(&done.tag, &done.body);
            let from = done.before.unwrap_or_default();
            if let Err(error) = hangar_update_news::record_component(&paths.home, Kind::ComputerControl, &from, &done.tag, vec![item]) {
                report(diary.as_deref(), "computer_control.novidade", &Error::plain(500, NEWS_FAILED, &error.to_string()));
            }
            Ok(Swapped::Yes { tag: done.tag })
        }
        Ok(None) => Ok(Swapped::No),
        Err(error) => {
            report(diary.as_deref(), "computer_control.atualizar", &error);
            Err(error)
        }
    })
    .await?
}

/// Sem cliente http a passada não instala nem atualiza: vai ao diário e fica como a falha da instalação,
/// que o `GET` mostra se o binário faltar.
pub fn client_failed(paths: &Paths, error: &str, diary: Option<&Path>) {
    let error = Error { status: 500, code: CLIENT_FAILED, msg: format!("sem cliente http: {error}"), params: json!({"error": error}) };
    remember_failure(paths, &error);
    report(diary, "computer_control.instalar", &error);
}

/// O código da última instalação automática que falhou, se ainda não houve uma que deu certo.
pub(crate) fn install_error(paths: &Paths) -> Option<String> {
    let bytes = std::fs::read(paths.install_error_file()).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("code")?.as_str().map(str::to_owned)
}

/// Baixa a release mais recente e a põe no lugar. `only_if_newer`: só troca por release mais nova
/// que a do `install.json`; sem tag lá é build própria e fica.
async fn latest(paths: &Paths, http: &reqwest::Client, github: &Github, only_if_newer: bool) -> Result<Option<Done>, Error> {
    let before = {
        let paths = paths.clone();
        off_thread(move || installed_tag(&paths)).await?
    };
    if only_if_newer && before.is_none() {
        return Ok(None);
    }
    let url = format!("{}/repos/{REPO}/releases/latest", github.api);
    let release = parse_release(&get(http, &url, RELEASE_TIMEOUT).await?).ok_or_else(|| {
        Error { status: 502, code: RELEASE_FAILED, msg: "a release mais recente veio sem tag".into(), params: json!({"error": "sem tag_name"}) }
    })?;
    if !TAG.is_match(&release.tag) {
        return Err(Error {
            status: 502,
            code: TAG_INVALID,
            msg: format!("a release mais recente veio com a tag inválida {:?}", release.tag),
            params: json!({"tag": release.tag}),
        });
    }
    if only_if_newer && let Some(installed) = &before {
        match newer(&release.tag, installed) {
            Some(true) => {}
            Some(false) => return Ok(None),
            None => {
                return Err(Error {
                    status: 502,
                    code: TAG_INCOMPARABLE,
                    msg: format!("não dá para comparar a release {} com a instalada {installed}", release.tag),
                    params: json!({"tag": release.tag, "installed": installed}),
                });
            }
        }
    }
    let required = required(paths, cfg!(windows));
    if let Some(name) = missing_asset(&release, &required) {
        return Err(Error {
            status: 502,
            code: NO_BINARY,
            msg: format!("a release {} não tem o binário {name}", release.tag),
            params: json!({"tag": release.tag}),
        });
    }
    let mut downloads = Vec::new();
    for (path, name, magic) in required {
        downloads.push((path, asset(http, github, &release.tag, name, magic).await?));
    }
    let (paths, tag) = (paths.clone(), release.tag.clone());
    let skipped = off_thread(move || put_in_place(&paths, downloads, &tag)).await??;
    Ok(Some(Done { tag: release.tag, body: release.body, before, skipped }))
}

/// Os arquivos que a plataforma usa, com o asset e o começo de cada um: no Windows o exe é o próprio
/// MCP (`Paths::binary`).
fn required(paths: &Paths, windows: bool) -> Vec<(PathBuf, &'static str, &'static [u8])> {
    let exe = (paths.exe(), EXE_ASSET, &b"MZ"[..]);
    if windows { vec![exe] } else { vec![(paths.install_dir().join(crate::NAME), LINUX_ASSET, &b"\x7fELF"[..]), exe] }
}

/// O primeiro asset que a plataforma usa e a release não lista.
fn missing_asset(release: &Release, required: &[(PathBuf, &'static str, &'static [u8])]) -> Option<&'static str> {
    required.iter().map(|(_, name, _)| *name).find(|name| !release.assets.iter().any(|asset| asset == name))
}

/// Um asset da release, conferido pelo começo do arquivo: página de erro com 200 ou corpo vazio
/// nunca toma o lugar do executável.
async fn asset(http: &reqwest::Client, github: &Github, tag: &str, name: &str, magic: &[u8]) -> Result<Vec<u8>, Error> {
    let url = format!("{}/{REPO}/releases/download/{tag}/{name}", github.download);
    let bytes = get(http, &url, ASSET_TIMEOUT).await?;
    if !bytes.starts_with(magic) {
        return Err(Error {
            status: 502,
            code: ASSET_INVALID,
            msg: format!("{url} não é um executável ({} bytes)", bytes.len()),
            params: json!({"asset": name, "size": bytes.len()}),
        });
    }
    Ok(bytes)
}

/// O `_get` do Python: os mesmos cabeçalhos em todo pedido; HTTP de erro é falha. Corpo acima de
/// `MAX_BODY` é recusado, pelo `Content-Length` ou durante a leitura.
async fn get(http: &reqwest::Client, url: &str, timeout: Duration) -> Result<Vec<u8>, Error> {
    let failed = |msg: String, error: String| Error { status: 502, code: RELEASE_FAILED, msg, params: json!({"error": error}) };
    let network = |error: reqwest::Error| failed(format!("não consegui baixar {url}: {error}"), error.to_string());
    let too_big = || Error {
        status: 502,
        code: ASSET_INVALID,
        msg: format!("{url} passa de {} MiB", MAX_BODY >> 20),
        params: json!({"error": "corpo grande demais"}),
    };
    let mut response = http
        .get(url)
        .header(reqwest::header::USER_AGENT, "hangar")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(timeout)
        .send()
        .await
        .map_err(network)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(failed(format!("o GitHub respondeu {status} em {url}"), format!("HTTP {status}")));
    }
    if response.content_length().is_some_and(|length| length > MAX_BODY) {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if (body.len() + chunk.len()) as u64 > MAX_BODY {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Tag, corpo e os nomes dos assets; sem `tag_name` texto não vazio é `None`.
fn parse_release(bytes: &[u8]) -> Option<Release> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let tag = value.get("tag_name")?.as_str().filter(|tag| !tag.is_empty())?.to_owned();
    let body = value.get("body").and_then(Value::as_str).unwrap_or("").to_owned();
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .map(|assets| assets.iter().filter_map(|asset| asset.get("name")?.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Some(Release { tag, body, assets })
}

/// `vX.Y.Z` por versão semântica (pré-release antes da final); `None` quando um lado não é versão.
fn newer(release: &str, installed: &str) -> Option<bool> {
    let parse = |tag: &str| semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok();
    // A metadata de build (`+…`) não conta, e `Prerelease` vazia é maior que qualquer pré-release.
    let key = |v: semver::Version| (v.major, v.minor, v.patch, v.pre);
    Some(key(parse(release)?) > key(parse(installed)?))
}

fn installed_tag(paths: &Paths) -> Option<String> {
    files::read_object(&paths.install_json(), false).ok()?.get("tag")?.as_str().map(str::to_owned)
}

/// Grava, troca, migra os alvos e só então o `install.json`. Devolve os alvos que não migraram.
fn put_in_place(paths: &Paths, downloads: Vec<(PathBuf, Vec<u8>)>, tag: &str) -> Result<Vec<String>, Error> {
    let dir = paths.install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| disk(&dir, &e))?;
    let mut staged = Vec::new();
    for (path, bytes) in downloads {
        let executable = !cfg!(windows) && path == paths.binary();
        match stage(&path, &bytes, executable) {
            Ok(temp) => staged.push((temp, path)),
            Err(error) => {
                for (temp, _) in &staged {
                    let _ = std::fs::remove_file(temp);
                }
                return Err(disk(&path, &error));
            }
        }
    }
    let swapped = swap_all(&staged)?;
    // Os alvos só migram com o `install.json` gravado: falhou, os binários voltam e os alvos nem mudaram.
    let written = std::fs::create_dir_all(paths.targets_dir()).map_err(|e| disk(&paths.targets_dir(), &e)).and_then(|()| {
        let install = paths.install_json();
        let text = pyjson::dumps_indent(&json!({"tag": tag}), 2, true);
        hangar_core::fs::atomic_write(&install, text.as_bytes()).map_err(|e| disk(&install, &e))
    });
    if let Err(error) = written {
        return Err(not_restored(error, undo(&swapped)));
    }
    Ok(migrate_targets(paths))
}

/// Temporário novo (nome aleatório, nunca reaproveitado) ao lado do destino, gravado no disco antes
/// da troca e já com o modo final.
#[cfg_attr(not(unix), allow(unused_variables))]
fn stage(path: &Path, bytes: &[u8], executable: bool) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.novo-{}", random_hex()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if executable {
            options.mode(0o755);
        }
    }
    let mut file = options.open(&temp)?;
    let written = file.write_all(bytes).and_then(|()| {
        // O `mode` da abertura passa pelo umask; o binário sai 0755 exato.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if executable {
                file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
            }
        }
        file.sync_all()
    });
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(temp)
}

/// Cada arquivo: o em uso vai para o lado (renomear funciona com o processo vivo, até no Windows,
/// onde substituir o exe em uso falha) e o novo toma o lugar. Falha desfaz os já trocados.
fn swap_all(staged: &[(PathBuf, PathBuf)]) -> Result<Vec<(PathBuf, Option<PathBuf>)>, Error> {
    let mut done = Vec::new();
    for (index, (temp, path)) in staged.iter().enumerate() {
        let mut failures = Vec::new();
        let aside = if path.is_file() { set_aside(path).map(Some) } else { Ok(None) };
        let placed = aside.and_then(|aside| {
            hangar_core::fs::replace(temp, path)
                .inspect_err(|_| {
                    if let Some(aside) = &aside
                        && let Err(error) = hangar_core::fs::replace(aside, path)
                    {
                        failures.push(restore_failure(path, &error));
                    }
                })
                .map(|()| aside)
        });
        match placed {
            Ok(aside) => done.push((path.clone(), aside)),
            Err(error) => {
                for (temp, _) in &staged[index..] {
                    let _ = std::fs::remove_file(temp);
                }
                failures.extend(undo(&done));
                return Err(not_restored(disk(path, &error), failures));
            }
        }
    }
    Ok(done)
}

/// Move o arquivo em uso para `<arquivo>.anterior`, a única cópia guardada. Se a de antes não sai
/// (no Windows, exe de uma sessão ainda aberta), vai para um nome único que o `tidy` apaga depois.
fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let previous = suffixed(path, ".anterior");
    let target = match std::fs::remove_file(&previous) {
        Ok(()) => previous,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => previous,
        Err(_) => suffixed(path, &format!(".anterior-{}", random_hex())),
    };
    hangar_core::fs::replace(path, &target)?;
    Ok(target)
}

/// Devolve a versão de antes; o arquivo que não existia sai. Devolve os que não voltaram.
fn undo(swapped: &[(PathBuf, Option<PathBuf>)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (path, aside) in swapped.iter().rev() {
        let restored = match aside {
            Some(aside) => hangar_core::fs::replace(aside, path),
            None => match std::fs::remove_file(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(error) = restored {
            failures.push(restore_failure(path, &error));
        }
    }
    failures
}

fn restore_failure(path: &Path, error: &std::io::Error) -> String {
    tracing::warn!(code = DISK_FAILED, path = %path.display(), %error, "Computer Use: versão anterior não voltou");
    format!("{}: {}", path.display(), crate::strerror(error))
}

/// A falha leva os arquivos que não voltaram à versão de antes (na mensagem, que vai ao diário, e em
/// `params.not_restored`).
fn not_restored(mut error: Error, failures: Vec<String>) -> Error {
    if failures.is_empty() {
        return error;
    }
    error.msg = format!("{}; não voltaram à versão anterior: {}", error.msg, failures.join(", "));
    if let Some(params) = error.params.as_object_mut() {
        params.insert("not_restored".into(), json!(failures));
    }
    error
}

/// Sobras de um tique interrompido (`.<arquivo>.novo-*`) e cópias `.anterior-*` postas de lado.
/// A que ainda está em uso não sai e fica para o próximo tique.
fn tidy(paths: &Paths) {
    let Ok(listing) = std::fs::read_dir(paths.install_dir()) else { return };
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with('.') && name.contains(".novo-")) || name.contains(".anterior-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn random_hex() -> String {
    use std::hash::BuildHasher;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:016x}", std::collections::hash_map::RandomState::new().hash_one((std::process::id(), nanos)))
}

/// O `_migrated` do Python em cada alvo: só regrava o que mudou. Ilegível, que não regravou ou que
/// ainda cita `linux_agent.py` volta na lista para o diário.
fn migrate_targets(paths: &Paths) -> Vec<String> {
    let exe = paths.exe().to_string_lossy().into_owned();
    let binary = paths.binary().to_string_lossy().into_owned();
    let mut skipped = Vec::new();
    let mut write_failed = false;
    for name in targets::target_files(paths) {
        let path = paths.targets_dir().join(&name);
        let Some(Value::Object(cfg)) = std::fs::read(&path).ok().and_then(|bytes| files::parse(&bytes)) else {
            tracing::warn!(alvo = %name, "Computer Use: alvo ilegível não migrou");
            skipped.push(name);
            continue;
        };
        let new = migrated(&cfg, &exe, &binary);
        if new != cfg {
            let text = pyjson::dumps_indent(&Value::Object(new), 2, true);
            if let Err(error) = hangar_core::fs::atomic_write(&path, text.as_bytes()) {
                tracing::warn!(alvo = %name, %error, "Computer Use: alvo não regravado");
                skipped.push(name);
                write_failed = true;
            }
        } else if pyjson::dumps(cfg.get("command").unwrap_or(&Value::Null), false).contains("linux_agent.py") {
            // Comando que a troca não reconheceu: continua no agente Python.
            skipped.push(name);
        }
    }
    let pending = paths.migration_pending_file();
    let marked = if write_failed { hangar_core::fs::atomic_write(&pending, b"") } else { remove_if_present(&pending) };
    if let Err(error) = marked {
        tracing::warn!(code = DISK_FAILED, %error, "Computer Use: pendência da migração não gravada");
    }
    skipped
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn migrated(cfg: &Map<String, Value>, exe: &str, binary: &str) -> Map<String, Value> {
    let with = |key: &str, value: Value| {
        let mut out = cfg.clone();
        out.insert(key.into(), value);
        out
    };
    if cfg.get("transport").and_then(Value::as_str) == Some("ssh")
        && cfg.get("agent_path").and_then(Value::as_str).is_some_and(|p| p.ends_with(EXE_ASSET))
    {
        return with("agent_path", json!(exe));
    }
    let Some(Value::Array(command)) = cfg.get("command") else { return cfg.clone() };
    // Sem `transport` vale local; `null` não.
    if cfg.get("transport").is_some_and(|t| t.as_str() != Some("local")) {
        return cfg.clone();
    }
    if let Some(first) = command.first() {
        let program = file_name(first.as_str().unwrap_or(""));
        let script = if command.len() > 1 && PYTHON.is_match(&program) { &command[1] } else { first };
        if file_name(script.as_str().unwrap_or("")) == "linux_agent.py" {
            return with("command", json!([binary, "agent"]));
        }
        // O wrapper sh fica: ele preenche HYPRLAND_INSTANCE_SIGNATURE, sem a qual o agente Rust não sobe.
        if command.len() >= 3 && matches!(program.as_str(), "sh" | "bash") && command[1] == "-c"
            && let Some(text) = command[2].as_str()
        {
            let wrapped = WRAPPED_AGENT.replacen(text, 1, NoExpand(&format!("exec {} agent", shell_quote(binary))));
            if wrapped != text {
                let mut new = command.clone();
                new[2] = json!(wrapped);
                return with("command", Value::Array(new));
            }
        }
        if first.as_str().is_some_and(|c| c.ends_with(EXE_ASSET)) {
            let mut new = command.clone();
            new[0] = json!(exe);
            return with("command", Value::Array(new));
        }
    }
    cfg.clone()
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// O `shlex.quote` do Python.
fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".into();
    }
    if text.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b)) {
        return text.into();
    }
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// O item no formato que os clientes leem; corpo vazio ou só o link `**Full Changelog**` fica só o título.
fn news_item(tag: &str, body: &str) -> Value {
    let body = body.trim();
    let only_changelog = body.lines().map(str::trim).filter(|l| !l.is_empty()).all(|l| l.starts_with("**Full Changelog**"));
    let text = if only_changelog { "" } else { body };
    json!({
        "id": format!("computer-control-{tag}"),
        "title": format!("Computer Use atualizado para {tag}"),
        "kind": "improvement",
        "text": text,
        "platforms": "all",
        "try_it": null,
        "animation": null,
    })
}

fn remember_failure(paths: &Paths, error: &Error) {
    let file = paths.install_error_file();
    let text = json!({"code": error.code, "msg": error.msg}).to_string();
    let written = std::fs::create_dir_all(paths.install_dir()).and_then(|()| hangar_core::fs::atomic_write(&file, text.as_bytes()));
    if let Err(error) = written {
        tracing::warn!(code = DISK_FAILED, %error, "Computer Use: falha da instalação não gravada");
    }
}

fn forget_failure(paths: &Paths) {
    match std::fs::remove_file(paths.install_error_file()) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(code = DISK_FAILED, %error, "Computer Use: falha antiga da instalação não apagada");
        }
        _ => {}
    }
}

fn report(diary: Option<&Path>, event: &str, error: &Error) {
    tracing::warn!(code = error.code, msg = %error.msg, "{event} falhou");
    let line = json!({"evento": event, "nivel": "erro", "codigo": error.code, "detalhe": error.msg});
    hangar_core::diag::record_blocking(diary, &[line], "computer control");
}

/// Sem resposta de rota: os alvos que não migraram vão ao diário.
fn report_skipped(diary: Option<&Path>, skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    tracing::warn!(code = "migration_skipped", alvos = %skipped.join(", "), "Computer Use: alvos não migrados");
    let line = json!({"evento": "computer_control.migracao", "nivel": "aviso", "codigo": "migration_skipped", "alvos": skipped.join(", ")});
    hangar_core::diag::record_blocking(diary, &[line], "computer control");
}

fn disk(path: &Path, error: &std::io::Error) -> Error {
    let error = crate::strerror(error);
    Error {
        status: 500,
        code: DISK_FAILED,
        msg: format!("{}: {error}", path.display()),
        params: json!({"file": path.to_string_lossy(), "error": error}),
    }
}

async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work).await.map_err(Error::io)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn release_parser_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = parse_release(&bytes);
        }

        #[test]
        fn release_parser_never_panics_on_json_shapes(tag in "\\PC{0,20}", body in "\\PC{0,40}", name in "\\PC{0,40}", shape in 0u8..4) {
            let assets = match shape { 0 => json!([{"name": name}]), 1 => json!({"name": name}), 2 => json!(name), _ => Value::Null };
            let release = json!({"tag_name": tag, "body": body, "assets": assets});
            let parsed = parse_release(release.to_string().as_bytes());
            prop_assert_eq!(parsed.is_some(), !tag.is_empty());
        }

        #[test]
        fn newer_never_panics(release in "\\PC{0,30}", installed in "\\PC{0,30}") {
            let _ = newer(&release, &installed);
        }

        #[test]
        fn shell_quote_is_safe_for_any_text(text in "\\PC{0,40}") {
            let quoted = shell_quote(&text);
            prop_assert!(!quoted.is_empty());
        }
    }

    /// `install.json` que não grava: os binários voltam e os alvos nem chegam a migrar.
    #[cfg(not(windows))]
    #[test]
    fn a_failed_install_json_leaves_the_targets_alone() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path().to_owned(), Vec::new());
        std::fs::create_dir_all(paths.targets_dir()).unwrap();
        std::fs::write(paths.binary(), b"\x7fELF-old").unwrap();
        std::fs::write(paths.exe(), b"MZ-old").unwrap();
        // Uma pasta no lugar do `install.json`: o rename por cima dela falha.
        std::fs::create_dir_all(paths.install_json()).unwrap();
        let target = paths.targets_dir().join("linux-agent.json");
        let old_target = br#"{"transport": "local", "command": ["python3", "/x/linux_agent.py"]}"#;
        std::fs::write(&target, old_target).unwrap();
        let downloads = vec![(paths.exe(), b"MZ-new".to_vec()), (paths.binary(), b"\x7fELF-new".to_vec())];
        let error = put_in_place(&paths, downloads, "v1.0.0").unwrap_err();
        assert_eq!(error.code, DISK_FAILED);
        assert_eq!(std::fs::read(&target).unwrap(), old_target);
        assert_eq!(std::fs::read(paths.binary()).unwrap(), b"\x7fELF-old");
        assert_eq!(std::fs::read(paths.exe()).unwrap(), b"MZ-old");
    }

    /// O desfazer devolve quem não voltou; arquivo novo que já sumiu não conta como falha.
    #[test]
    fn undo_lists_what_did_not_come_back() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("hangar-computer-control");
        std::fs::write(&path, b"novo").unwrap();
        let failures = undo(&[(path.clone(), Some(home.path().join("sumiu.anterior"))), (home.path().join("nunca-existiu"), None)]);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].starts_with(&path.display().to_string()), "{failures:?}");
        let error = not_restored(Error::plain(500, DISK_FAILED, "falhou"), failures.clone());
        assert!(error.msg.starts_with("falhou; não voltaram à versão anterior: "), "{}", error.msg);
        assert_eq!(error.params["not_restored"], json!(failures));
        assert_eq!(not_restored(Error::plain(500, DISK_FAILED, "falhou"), Vec::new()).msg, "falhou");
    }

    /// Sem cliente http: falha da instalação gravada com código e linha no diário.
    #[test]
    fn a_missing_http_client_is_remembered_and_reported() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path().to_owned(), Vec::new());
        let diary = home.path().join("diario");
        client_failed(&paths, "tls", Some(&diary));
        assert_eq!(install_error(&paths).as_deref(), Some(CLIENT_FAILED));
        let lines: String = std::fs::read_dir(&diary).unwrap().flatten().map(|f| std::fs::read_to_string(f.path()).unwrap()).collect();
        assert!(lines.contains(CLIENT_FAILED), "{lines}");
    }

    #[test]
    fn release_without_string_tag_is_none() {
        for bytes in [&b"[]"[..], br#"{"tag_name": 1}"#, br#"{"tag_name": ""}"#, br#"{}"#, b"not json"] {
            assert_eq!(parse_release(bytes), None);
        }
        let release = parse_release(br#"{"tag_name": "v1", "body": null, "assets": [{"name": "hangar-computer-control-linux-x86_64"}]}"#);
        let assets = vec!["hangar-computer-control-linux-x86_64".to_owned()];
        assert_eq!(release, Some(Release { tag: "v1".into(), body: String::new(), assets }));
    }

    #[test]
    fn required_files_follow_the_platform() {
        let paths = Paths::new(PathBuf::from("/h"), Vec::new());
        let files = |windows| required(&paths, windows).into_iter().map(|(path, ..)| path).collect::<Vec<_>>();
        assert_eq!(files(true), [paths.exe()]);
        assert_eq!(files(false), [paths.install_dir().join(crate::NAME), paths.exe()]);
        assert_eq!(files(cfg!(windows))[0], paths.binary());
    }

    /// No Windows só o exe é baixado: release sem o binário Linux instala; no Linux ela falta.
    #[test]
    fn the_missing_asset_follows_the_platform() {
        let paths = Paths::new(PathBuf::from("/h"), Vec::new());
        let release = Release { tag: "v1.0.0".into(), body: String::new(), assets: vec![EXE_ASSET.into()] };
        assert_eq!(missing_asset(&release, &required(&paths, true)), None);
        assert_eq!(missing_asset(&release, &required(&paths, false)), Some(LINUX_ASSET));
        let empty = Release { assets: Vec::new(), ..release };
        assert_eq!(missing_asset(&empty, &required(&paths, true)), Some(EXE_ASSET));
    }

    #[test]
    fn tag_accepts_only_versions() {
        for tag in ["v0.7.0", "0.7.0", "v1.2.3-rc.1", "v1.2.3+build-5", "v10.20.30"] {
            assert!(TAG.is_match(tag), "{tag}");
        }
        for tag in ["", "nightly", "v1.2", "v1.2.3/../x", "v1.2.3?x", "v1.2.3-", "v1.2.3 ", "v1.2.3\n", "v١.2.3", "33c5fbf"] {
            assert!(!TAG.is_match(tag), "{tag:?}");
        }
    }

    #[test]
    fn newer_compares_semantic_versions() {
        assert_eq!(newer("v0.10.0", "v0.9.9"), Some(true));
        assert_eq!(newer("v0.7.0", "v0.7.0"), Some(false));
        assert_eq!(newer("v0.6.9", "v0.7.0"), Some(false));
        assert_eq!(newer("v0.8.0", "v0.8.0-rc.1"), Some(true));
        assert_eq!(newer("v0.8.0-rc.1", "v0.8.0"), Some(false));
        assert_eq!(newer("v0.8.0-rc.2", "v0.8.0-rc.1"), Some(true));
        assert_eq!(newer("v0.8.0-rc.1", "v0.7.0"), Some(true));
        assert_eq!(newer("v0.8.0+build.2", "v0.8.0+build.1"), Some(false));
        assert_eq!(newer("0.8.0", "v0.7.0"), Some(true));
        assert_eq!(newer("nightly", "v0.7.0"), None);
        assert_eq!(newer("v0.8.0", "33c5fbf"), None);
        assert_eq!(newer("v0.8", "v0.7.0"), None);
    }

    #[test]
    fn shell_quote_matches_python() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("/home/u/.hangar/hcu"), "/home/u/.hangar/hcu");
        assert_eq!(shell_quote("/a b/it's"), r#"'/a b/it'"'"'s'"#);
        assert_eq!(shell_quote("/home/joão"), "'/home/joão'");
    }

    #[test]
    fn news_text_drops_a_body_that_is_only_the_changelog_link() {
        let only = "**Full Changelog**: https://github.com/jeffer1312/hangar-computer-control/compare/v0.6.0...v0.7.0";
        assert_eq!(news_item("v0.7.0", only)["text"], "");
        assert_eq!(news_item("v0.7.0", "  \n")["text"], "");
        let item = news_item("v0.7.0", "Agente mais rápido.\n\n");
        assert_eq!(item, json!({
            "id": "computer-control-v0.7.0", "title": "Computer Use atualizado para v0.7.0", "kind": "improvement",
            "text": "Agente mais rápido.", "platforms": "all", "try_it": null, "animation": null
        }));
        assert_eq!(news_item("v1", &format!("Novo.\n\n{only}"))["text"], format!("Novo.\n\n{only}"));
    }

    #[test]
    fn migrated_follows_the_python_rules() {
        let (exe, bin) = ("/h/windows-agent.exe", "/h/hangar-computer-control");
        let obj = |v: Value| v.as_object().unwrap().clone();
        let cases = [
            (json!({"transport": "ssh", "host": "h", "agent_path": "C:/x/windows-agent.exe"}),
             json!({"transport": "ssh", "host": "h", "agent_path": exe})),
            (json!({"transport": "local", "command": ["python3", "/x/linux_agent.py"], "request_timeout": 15}),
             json!({"transport": "local", "command": [bin, "agent"], "request_timeout": 15})),
            (json!({"command": ["/x/linux_agent.py"]}), json!({"command": [bin, "agent"]})),
            (json!({"transport": "local", "command": ["/opt/run.sh", "linux_agent.py"]}),
             json!({"transport": "local", "command": ["/opt/run.sh", "linux_agent.py"]})),
            (json!({"transport": "local", "command": ["sh", "-c", "export X=1; exec /usr/bin/python3.12 /o/linux_agent.py --v"]}),
             json!({"transport": "local", "command": ["sh", "-c", format!("export X=1; exec {bin} agent --v")]})),
            (json!({"transport": "local", "command": ["C:/old/windows-agent.exe", "--x"]}),
             json!({"transport": "local", "command": [exe, "--x"]})),
            (json!({"transport": null, "command": ["/x/linux_agent.py"]}), json!({"transport": null, "command": ["/x/linux_agent.py"]})),
            (json!({"transport": "local", "command": []}), json!({"transport": "local", "command": []})),
        ];
        for (before, after) in cases {
            assert_eq!(Value::Object(migrated(&obj(before.clone()), exe, bin)), after, "{before}");
        }
    }

    /// ROT-80, ROT-82: os prazos do `_get` do Python, 15 s para `releases/latest` e 120 s por asset.
    #[test]
    fn github_deadlines_match_the_python() {
        assert_eq!(RELEASE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(ASSET_TIMEOUT, Duration::from_secs(120));
    }
}
