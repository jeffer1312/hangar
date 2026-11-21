//! Atualizador por pacote: lê o índice da release, baixa e confere o pacote, extrai lado a lado
//! em `~/.hangar/versions/<versao>`, troca o `current`, roda os passos de máquina, reinicia e só dá
//! por feito com o servidor de pé em modo `rust`. Falha antes do reinício só devolve o `current`;
//! depois dele, o `current` volta e o servidor anterior reinicia.
//!
//! Biblioteca sem rota: quem reinicia (`Restarter`), quem prova (`HealthProbe`), quem roda passo
//! (`StepRunner`) e quem diz se uma pasta está em uso (`InUse`) entram de fora.
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hangar_local_server::link::{self, real};
use hangar_package::{ExtractError, Manifest, PackageIndex, PlatformEntry, Version, MANIFEST_FILE};
use serde_json::{json, Value};

pub(crate) use hangar_local_server::link::remove_link;

pub mod dev;
pub mod dev_mode;
pub mod executor;
pub mod http;
pub mod git_engine;
pub mod install;
pub mod migrate;
pub mod periodic;
pub mod process;
pub mod restart;
pub mod startup_output;
pub mod state;
pub mod steps;
pub mod stop_intent;

#[cfg(test)]
mod tests;

pub use state::{state_dir, StateFile, StateSnapshot};
pub use steps::{apply_machine_steps, ShellRunner, StepRun, StepRunner};

/// Onde o pacote mora: `~/.hangar`.
#[derive(Debug, Clone)]
pub struct Layout {
    pub hangar: PathBuf,
}

impl Layout {
    pub fn new(home: &Path) -> Layout { Layout { hangar: home.join(".hangar") } }
    pub fn versions(&self) -> PathBuf { self.hangar.join("versions") }
    pub fn current(&self) -> PathBuf { self.hangar.join("current") }
    /// Onde a troca do Windows põe o `current` velho entre os dois `rename`.
    pub fn aside(&self) -> PathBuf { link::aside(&self.hangar) }
    pub fn downloads(&self) -> PathBuf { self.hangar.join("downloads") }
    pub fn lock_file(&self) -> PathBuf { hangar_local_server::lock::lock_file(&self.hangar) }
    /// A troca feita e ainda sem prova (`link::Unproven`).
    pub fn unproven(&self) -> PathBuf { link::unproven_file(&self.hangar) }
}

/// A release do pacote: `<base>/package-<canal>/package-<canal>.json` e os `.tar.gz` ao lado.
#[derive(Debug, Clone)]
pub struct Release {
    pub base_url: String,
    pub channel: String,
}

/// Onde as releases do pacote moram.
pub const RELEASE_BASE: &str = "https://github.com/jeffer1312/hangar/releases/download";

impl Release {
    pub fn github(channel: &str) -> Release {
        Release { base_url: RELEASE_BASE.into(), channel: channel.into() }
    }
    pub fn index_url(&self) -> String { format!("{}/package-{}/package-{}.json", self.base_url, self.channel, self.channel) }
    pub fn asset_url(&self, asset: &str) -> String { format!("{}/package-{}/{asset}", self.base_url, self.channel) }
}

/// Chave do sistema no índice: `linux-x86_64`, `windows-x86_64`.
pub fn platform() -> String { format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH) }

#[derive(Debug)]
pub enum UpdateError {
    Locked,
    Io(String),
    Index(String),
    NoPlatform(String),
    Download(String),
    Sha { expected: String, got: String },
    Package(ExtractError),
    Step(String),
    Swap(String),
}

impl UpdateError {
    pub fn code(&self) -> &'static str {
        match self {
            UpdateError::Locked => "update_locked",
            UpdateError::Io(_) => "update_io",
            UpdateError::Index(_) => "update_index_unavailable",
            UpdateError::NoPlatform(_) => "update_no_platform",
            UpdateError::Download(_) => "update_download_failed",
            UpdateError::Sha { .. } => "update_sha_mismatch",
            UpdateError::Package(e) => e.code(),
            UpdateError::Step(_) => "update_step_failed",
            UpdateError::Swap(_) => "update_swap_failed",
        }
    }
}

/// O texto vai ao `erro` do estado, que a tela mostra.
impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Locked => write!(f, "já existe uma atualização rodando"),
            UpdateError::Io(d) => f.write_str(d),
            UpdateError::Index(d) => write!(f, "não consegui ler o índice da release: {d}"),
            UpdateError::NoPlatform(p) => write!(f, "a release não tem pacote para este sistema ({p})"),
            UpdateError::Download(d) => write!(f, "o download do pacote falhou: {d}"),
            UpdateError::Sha { expected, got } => write!(f, "o pacote baixado não confere com o publicado (sha256 {got}, esperado {expected})"),
            UpdateError::Package(e) => write!(f, "o pacote não foi extraído: {e}"),
            UpdateError::Step(d) => f.write_str(d),
            UpdateError::Swap(d) => write!(f, "não consegui trocar a versão em uso: {d}"),
        }
    }
}

impl std::error::Error for UpdateError {}

/// Uma atualização por vez na máquina: a trava de `~/.hangar/update.lock`, a mesma que a troca de
/// versão do app nativo segura (`hangar_local_server::lock`).
pub struct UpdateLock {
    _lock: hangar_local_server::UpdateLock,
}

impl UpdateLock {
    pub fn try_acquire(layout: &Layout) -> Result<UpdateLock, UpdateError> {
        match hangar_local_server::UpdateLock::try_acquire(&layout.hangar) {
            Ok(lock) => Ok(UpdateLock { _lock: lock }),
            Err(hangar_local_server::LockError::Locked) => Err(UpdateError::Locked),
            Err(hangar_local_server::LockError::Io(e)) => Err(UpdateError::Io(format!("trava da atualização ({}): {e}", layout.lock_file().display()))),
        }
    }
}

fn read_manifest(dir: &Path) -> Option<Manifest> { Manifest::parse(&std::fs::read(dir.join(MANIFEST_FILE)).ok()?).ok() }

/// É pacote: o critério mora no crate do servidor parado, que o app nativo também usa.
pub use hangar_local_server::versions::{is_package, package_manifest};

/// O manifesto em uso marca pacote mas não serve ao atualizador: o motivo, para o estado e a tela.
pub fn broken_manifest(layout: &Layout) -> Option<String> {
    let path = layout.current().join(MANIFEST_FILE);
    if !is_package(&layout.current()) { return None; }
    let error = Manifest::parse(&std::fs::read(&path).ok()?).err()?;
    Some(format!("o manifest.json da versão em uso ({}) está incompleto para atualizar: {error}; reinstale o pacote", path.display()))
}

/// A versão para onde o `current` aponta, pelo `manifest.json` dela; `None` sem pacote (modo
/// desenvolvedor, máquina não migrada).
pub fn installed(layout: &Layout) -> Option<Manifest> { read_manifest(&layout.current()) }

/// Avisos importantes que a pessoa aceitou na tela (ids). O passo destrutivo só roda com o dele aqui;
/// o `scripts/migrar-pacote.*` lê o mesmo arquivo.
pub fn accepted_file(layout: &Layout) -> PathBuf { layout.hangar.join("update-accepted.json") }

/// O passo da migração rodou sem o aviso aceito: só deixou esta marca, e a migração espera a tela.
pub fn offered_file(layout: &Layout) -> PathBuf { layout.hangar.join("migration-offered") }

/// Ilegível é vazio: o aviso volta à tela, e nada destrutivo roda sem aceite.
pub fn accepted(layout: &Layout) -> std::collections::BTreeSet<String> {
    std::fs::read(accepted_file(layout)).ok().and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default().into_iter().collect()
}

pub fn accept(layout: &Layout, ids: &[String]) -> std::io::Result<()> {
    let mut all = accepted(layout);
    all.extend(ids.iter().cloned());
    std::fs::create_dir_all(&layout.hangar)?;
    crate::runtime::queue::atomic_write(&accepted_file(layout), json!(all).to_string().as_bytes())
}

/// Desfaz o aceite de `ids`: o que ele devia lançar não subiu, e o aviso tem de voltar à tela.
pub fn unaccept(layout: &Layout, ids: &[String]) -> std::io::Result<()> {
    let all: Vec<String> = accepted(layout).into_iter().filter(|id| !ids.contains(id)).collect();
    crate::runtime::queue::atomic_write(&accepted_file(layout), json!(all).to_string().as_bytes())
}

/// O Canal de testes do pacote, escolhido pela tela: `{"channel": "beta", "last": "beta"}`.
pub fn channel_file(layout: &Layout) -> PathBuf { layout.hangar.join("config").join("update-channel.json") }

/// A escolha gravada: `(canal, último canal de testes)`.
pub fn chosen_channel(layout: &Layout) -> Option<(String, String)> {
    let value: Value = serde_json::from_slice(&std::fs::read(channel_file(layout)).ok()?).ok()?;
    let channel = value.get("channel")?.as_str().filter(|c| !c.is_empty())?.to_owned();
    let last = value.get("last").and_then(Value::as_str).unwrap_or_default().to_owned();
    Some((channel, last))
}

pub fn save_channel(layout: &Layout, channel: &str, last: &str) -> std::io::Result<()> {
    let file = channel_file(layout);
    if let Some(parent) = file.parent() { std::fs::create_dir_all(parent)?; }
    crate::runtime::queue::atomic_write(&file, json!({"channel": channel, "last": last}).to_string().as_bytes())
}

/// O canal que o Atualizar segue: o escolhido na tela, senão o do pacote instalado, senão `latest`.
pub fn channel(layout: &Layout) -> String {
    chosen_channel(layout).map(|(c, _)| c).or_else(|| installed(layout).map(|m| m.channel)).unwrap_or_else(|| "latest".to_owned())
}

/// A entrada deste sistema quando é mais nova que a instalada (ou quando não há instalada).
pub async fn check(client: &reqwest::Client, release: &Release, platform: &str, installed: Option<Version>)
    -> Result<Option<PlatformEntry>, UpdateError> {
    let entry = entry(client, release, platform).await?;
    let version: Version = entry.version.parse().map_err(|e: ExtractError| UpdateError::Index(e.to_string()))?;
    Ok(installed.is_none_or(|installed| version > installed).then_some(entry))
}

/// A entrada deste sistema no índice publicado, mais nova ou não.
pub async fn entry(client: &reqwest::Client, release: &Release, platform: &str) -> Result<PlatformEntry, UpdateError> {
    // Queda de rede no meio do corpo não vira atualização falha; recusa do servidor (4xx) não muda tentando de novo.
    // O total cabe nos 120 s em que a tela desiste do `?procurar=1`, com folga para a resposta.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(110);
    let mut waits = [Duration::from_secs(2), Duration::from_secs(5)].into_iter();
    let index = loop {
        let limit = deadline.saturating_duration_since(tokio::time::Instant::now()).min(Duration::from_secs(60));
        match read_index(client, release, limit).await {
            Ok(index) => break index,
            Err((error, retry)) => match waits.next()
                .filter(|wait| retry && deadline.saturating_duration_since(tokio::time::Instant::now()) > *wait + Duration::from_secs(5)) {
                Some(wait) => {
                    tracing::warn!(code = "update_index_retry", error = %error, wait_s = wait.as_secs(), "índice da release não lido; tentando de novo");
                    tokio::time::sleep(wait).await;
                }
                None => return Err(error),
            },
        }
    };
    index.platforms.get(platform).cloned().ok_or_else(|| UpdateError::NoPlatform(platform.to_owned()))
}

/// O índice publicado; o erro diz se vale tentar de novo. Só a rede falha passageira: laço de
/// redirecionamento e índice fora do formato dão o mesmo resultado na tentativa seguinte, mas o
/// JSON que acaba no meio é corpo cortado sem `Content-Length`.
async fn read_index(client: &reqwest::Client, release: &Release, limit: Duration) -> Result<PackageIndex, (UpdateError, bool)> {
    let url = release.index_url();
    let response = client.get(&url).timeout(limit).send().await.map_err(|e| { let retry = !e.is_redirect(); (UpdateError::Index(e.to_string()), retry) })?;
    if !response.status().is_success() {
        return Err((UpdateError::Index(format!("{} em {url}", response.status())), !response.status().is_client_error()));
    }
    let bytes = response.bytes().await.map_err(|e| (UpdateError::Index(e.to_string()), true))?;
    serde_json::from_slice(&bytes).map_err(|e| { let retry = e.is_eof(); (UpdateError::Index(e.to_string()), retry) })
}

/// Baixa com prazo, gravando e calculando o sha256 ao mesmo tempo.
async fn download(client: &reqwest::Client, url: &str, dest: &Path, deadline: Duration) -> Result<String, UpdateError> {
    let fail = |e: &dyn std::fmt::Display| UpdateError::Download(e.to_string());
    let mut response = client.get(url).timeout(deadline).send().await.map_err(|e| fail(&e))?;
    if !response.status().is_success() {
        return Err(UpdateError::Download(format!("{} em {url}", response.status())));
    }
    let mut out = std::fs::File::create(dest).map_err(|e| fail(&e))?;
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    while let Some(chunk) = response.chunk().await.map_err(|e| fail(&e))? {
        sha.update(&chunk);
        out.write_all(&chunk).map_err(|e| fail(&e))?;
    }
    out.sync_all().map_err(|e| fail(&e))?;
    Ok(sha.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

/// Baixa o pacote com prazo, confere o sha256 do índice e extrai em `versions/<versao>`. Pasta que
/// já existe e não confere é apagada e refeita, menos a do `current`.
pub async fn prepare(client: &reqwest::Client, release: &Release, entry: &PlatformEntry, layout: &Layout, deadline: Duration)
    -> Result<PathBuf, UpdateError> {
    // O nome vem da rede: só nome de arquivo, nunca caminho.
    if entry.asset.is_empty() || entry.asset.starts_with('.') || Path::new(&entry.asset).file_name() != Some(entry.asset.as_ref())
        || entry.asset.contains(['/', '\\', ':']) {
        return Err(UpdateError::Download(format!("nome de pacote inválido no índice: {:?}", entry.asset)));
    }
    let io = |e: std::io::Error| UpdateError::Io(e.to_string());
    std::fs::create_dir_all(layout.downloads()).map_err(io)?;
    std::fs::create_dir_all(layout.versions()).map_err(io)?;
    let archive = layout.downloads().join(&entry.asset);
    let result = async {
        let got = download(client, &release.asset_url(&entry.asset), &archive, deadline).await?;
        if !got.eq_ignore_ascii_case(&entry.sha256) {
            return Err(UpdateError::Sha { expected: entry.sha256.clone(), got });
        }
        let (layout, archive, version) = (layout.clone(), archive.clone(), entry.version.clone());
        blocking(move || extract(&layout, &archive, &version)).await
    }.await;
    // Melhor esforço: o erro que importa é o da rodada, e o próximo download sobrescreve.
    let _ = std::fs::remove_file(&archive);
    result
}

fn extract(layout: &Layout, archive: &Path, version: &str) -> Result<PathBuf, UpdateError> {
    let dir = match hangar_package::extract(archive, &layout.versions()) {
        Err(ExtractError::ExistingDiffers(why)) => {
            let dir = layout.versions().join(version);
            if is_current(layout, &dir) {
                return Err(UpdateError::Package(ExtractError::ExistingDiffers(why)));
            }
            let trash = to_trash(layout, &dir).map_err(|e| UpdateError::Io(format!("a pasta {} não confere e não saiu do lugar: {e}", dir.display())))?;
            let _ = std::fs::remove_dir_all(trash);
            hangar_package::extract(archive, &layout.versions()).map_err(UpdateError::Package)?
        }
        other => other.map_err(UpdateError::Package)?,
    };
    if dir.file_name() != Some(version.as_ref()) {
        return Err(UpdateError::Package(ExtractError::ManifestInvalid(format!("o índice anuncia {version}, o pacote é {}", dir.display()))));
    }
    Ok(dir)
}

fn is_current(layout: &Layout, dir: &Path) -> bool {
    matches!((real(&layout.current()), real(dir)), (Ok(a), Ok(b)) if a == b)
}

/// Tira a pasta do lugar para `versions/.trash-<n>`: uma pasta pela metade nunca fica com nome de
/// versão.
fn to_trash(layout: &Layout, dir: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let trash = layout.versions().join(format!(".trash-{}-{stamp}", std::process::id()));
    std::fs::rename(dir, &trash)?;
    Ok(trash)
}

/// Aponta o `current` para `target` e devolve para onde ele apontava (caminho real), para a volta.
/// Linux: link novo e `rename` atômico por cima. Windows: junction nova ao lado, a velha renomeada
/// para o lado e a nova renomeada para o lugar.
pub fn swap(layout: &Layout, target: &Path) -> Result<Option<PathBuf>, UpdateError> {
    link::swap(&layout.hangar, target).map_err(|e| UpdateError::Swap(e.to_string()))
}

/// Rodada interrompida no meio: a troca do Windows entre os dois `rename` (o link de antes volta
/// ao lugar) ou a troca feita e nunca provada (o `current` volta para a versão anterior, como a
/// volta da rodada faria). `true` quando havia o que recuperar.
pub fn recover(layout: &Layout) -> std::io::Result<bool> {
    let aside = link::recover_aside(&layout.hangar)?;
    let Some(record) = link::read_unproven(&layout.hangar)? else { return Ok(aside) };
    // O `current` em outra versão: a troca não chegou a acontecer, ou alguém já a desfez.
    if real(&layout.current()).ok() != Some(record.target) {
        clear_unproven(layout)?;
        return Ok(aside);
    }
    match &record.previous {
        Some(previous) => { swap(layout, previous).map_err(|e| std::io::Error::other(e.to_string()))?; }
        None => remove_link(&layout.current())?,
    }
    clear_unproven(layout)?;
    Ok(true)
}

/// A troca que a rodada fez e ainda não provou, gravada antes dela: a rodada que morre (executor
/// morto, queda de energia, passo travado) deixa o que desfazer. A tela do servidor parado lê o
/// mesmo registro para não oferecer a versão que falhou.
fn note_unproven(layout: &Layout, target: &Path) -> Result<(), UpdateError> {
    let io = |e: &dyn std::fmt::Display| UpdateError::Io(format!("não consegui gravar {}: {e}", layout.unproven().display()));
    let record = link::Unproven { previous: real(&layout.current()).ok(), target: real(target).map_err(|e| io(&e))? };
    crate::runtime::queue::atomic_write(&layout.unproven(), &serde_json::to_vec(&record).map_err(|e| io(&e))?).map_err(|e| io(&e))
}

fn clear_unproven(layout: &Layout) -> std::io::Result<()> { link::remove_unproven(&layout.hangar) }

/// Diz se algum processo vivo usa a pasta.
pub trait InUse {
    fn in_use(&self, dir: &Path) -> bool;
}

/// Em uso de verdade. Linux: algum processo do `/proc` com `exe`, `cwd` ou arquivo mapeado
/// (`maps`) dentro da pasta; processo que não se deixa ler (outro usuário) é ignorado. Fora do
/// Linux, não: no Windows quem a poda conta como em uso é o acesso negado, na troca de nome
/// (processo com a pasta como `cwd`) ou no apagar (executável rodando de dentro dela, que a troca
/// de nome deixa passar).
pub struct ProcInUse;

impl InUse for ProcInUse {
    #[cfg(target_os = "linux")]
    fn in_use(&self, dir: &Path) -> bool {
        let Ok(dir) = real(dir) else { return false };
        let Ok(procs) = std::fs::read_dir("/proc") else { return false };
        procs.flatten().filter(|p| p.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))).any(|p| {
            let base = p.path();
            ["exe", "cwd"].iter().any(|link| std::fs::read_link(base.join(link)).is_ok_and(|t| t.starts_with(&dir)))
                || std::fs::read_to_string(base.join("maps")).is_ok_and(|maps| maps.lines()
                    .any(|line| line.find('/').is_some_and(|at| Path::new(&line[at..]).starts_with(&dir))))
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn in_use(&self, _dir: &Path) -> bool { false }
}

/// O que a poda fez: apagadas, puladas por uso e falhas (vão aos avisos).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub removed: Vec<PathBuf>,
    pub in_use: Vec<PathBuf>,
    pub failed: Vec<String>,
}

/// Acesso negado (5) e arquivo em uso (32): no Windows é o processo vivo dentro da pasta.
fn held_by_process(e: &std::io::Error) -> bool { cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32)) }

/// Apaga as versões que não são a do `current` nem `previous` (as duas que a volta usa), menos as
/// que um processo vivo usa. Sobra de poda anterior (`.trash-*`) é apagada de novo.
pub fn prune(layout: &Layout, previous: Option<&Path>, in_use: &dyn InUse) -> Pruned {
    let mut out = Pruned::default();
    let keep: Vec<PathBuf> = [real(&layout.current()).ok(), previous.and_then(|p| real(p).ok())].into_iter().flatten().collect();
    let Ok(entries) = std::fs::read_dir(layout.versions()) else { return out };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".trash-") {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        // Só pasta de versão de verdade: `.tmp` de extração e links ficam.
        if name.parse::<Version>().is_err() || !entry.file_type().is_ok_and(|t| t.is_dir()) { continue; }
        if real(&path).is_ok_and(|r| keep.contains(&r)) { continue; }
        if in_use.in_use(&path) {
            out.in_use.push(path);
            continue;
        }
        match to_trash(layout, &path) {
            Ok(trash) => match std::fs::remove_dir_all(&trash) {
                Ok(()) => out.removed.push(path),
                // A sobra `.trash-*` sai numa poda seguinte, quando o processo tiver saído.
                Err(e) if held_by_process(&e) => out.in_use.push(path),
                Err(e) => out.failed.push(format!("a versão {name} saiu do lugar mas não foi apagada: {e}")),
            },
            Err(e) if held_by_process(&e) => out.in_use.push(path),
            Err(e) => out.failed.push(format!("a versão {name} não foi podada: {e}")),
        }
    }
    out
}

/// Uma leitura da prova de vida.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// Ninguém respondeu.
    Down(String),
    /// O `hangar-server` respondeu, com o contrato e o pid dele.
    Rust { protocol: u32, pid: Option<u32>, commit: Option<String> },
}

pub trait HealthProbe {
    fn read(&self) -> impl Future<Output = Reading> + Send;
}

pub trait Restarter {
    /// O reiniciador real grava a marca de intenção com `why` antes de derrubar o servidor e a
    /// acerta se o reinício falhar.
    fn restart(&self, why: stop_intent::RestartIntent) -> impl Future<Output = Result<(), String>> + Send;
    /// Instalação na mão: não há serviço a reiniciar; a versão nova entra no próximo reinício dela.
    fn manual(&self) -> bool { false }
}

/// Baixa a tela web (`frontend/dist`) da chave para `ui_dir`; o real é o `ui::fetch`.
/// Baixa e prepara a versão publicada no canal, sem trocar o `current`: a volta do modo
/// desenvolvedor para o pacote numa máquina que nunca teve um. Devolve o aviso da tela web, se falhou.
pub async fn download_published(release: &Release, layout: &Layout, home: &Path, settings: &Settings) -> Result<Option<String>, UpdateError> {
    let client = crate::http_client().build().map_err(|e| UpdateError::Download(e.to_string()))?;
    let entry = entry(&client, release, &platform()).await?;
    let dir = prepare(&client, release, &entry, layout, settings.download_deadline).await?;
    let key = read_manifest(&dir).and_then(|m| m.dist_key);
    // Escolha ilegível: a tela fica ligada, como na atualização.
    if let Some(key) = key && crate::ui::enabled(home).unwrap_or(true)
        && let Err(e) = ReleaseUi.fetch(&key, &layout.hangar.join("ui")).await {
        return Ok(Some(format!("a tela web não foi baixada: {e}")));
    }
    Ok(None)
}

pub trait UiFetcher: Sync {
    fn fetch<'a>(&'a self, key: &'a str, ui_dir: &'a Path) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

/// A tela pela release `dist-commits`.
pub struct ReleaseUi;

impl UiFetcher for ReleaseUi {
    fn fetch<'a>(&'a self, key: &'a str, ui_dir: &'a Path) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { crate::ui::fetch(key, ui_dir).await.map(|_| ()).map_err(|e| e.to_string()) })
    }
}

/// A prova de vida real: `GET <base>/__hangar_server/health`, que só o `hangar-server` atende.
pub struct HttpHealth {
    pub client: reqwest::Client,
    pub url: String,
}

impl HttpHealth {
    pub fn new(base_url: &str) -> HttpHealth {
        let client = crate::http_client().timeout(Duration::from_secs(3)).build().unwrap_or_default();
        HttpHealth { client, url: format!("{}/__hangar_server/health", base_url.trim_end_matches('/')) }
    }
}

impl HealthProbe for HttpHealth {
    async fn read(&self) -> Reading {
        let response = match self.client.get(&self.url).send().await {
            Ok(response) => response,
            // A causa separa ninguém escutando (recusada) de processo de pé e travado (tempo esgotado).
            Err(e) => return Reading::Down(crate::groups::peers::chain(&e)),
        };
        let status = response.status();
        let body: Value = response.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        match body["protocol"].as_u64().and_then(|p| u32::try_from(p).ok()) {
            Some(protocol) if status.is_success() => Reading::Rust { protocol, pid: body["pid"].as_u64().and_then(|p| u32::try_from(p).ok()),
                commit: body["package_commit"].as_str().map(str::to_owned) },
            // Outro programa na porta: para a prova de vida, o servidor não respondeu.
            _ => Reading::Down(format!("{status} em {} sem o hangar-server", self.url)),
        }
    }
}

/// Prazos da rodada.
#[derive(Debug, Clone)]
pub struct Settings {
    pub download_deadline: Duration,
    pub health_deadline: Duration,
    pub health_interval: Duration,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { download_deadline: Duration::from_secs(900), health_deadline: Duration::from_secs(90), health_interval: Duration::from_secs(1) }
    }
}

/// Tudo o que uma rodada precisa saber.
pub struct Job<'a> {
    pub client: &'a reqwest::Client,
    pub release: &'a Release,
    pub platform: &'a str,
    pub layout: &'a Layout,
    pub home: &'a Path,
    /// `<config do Claude>/.hangar-update`: estado e registro de passos.
    pub state_dir: &'a Path,
    /// Pasta do diário exportável (`diag::diary_dir()`); `None` não grava.
    pub diary_dir: Option<PathBuf>,
    /// Quem baixa a tela web da versão nova; `None` não baixa.
    pub ui: Option<&'a dyn UiFetcher>,
    /// Avisos do lançamento, gravados depois que a rodada zera os avisos.
    pub notes: Vec<String>,
    pub settings: Settings,
    /// A porta pública: com ela, a falha do reinício leva a saída da versão nova e o retrato da
    /// máquina (`startup_output`). `None` nos testes, que não leem o journal nem os processos da máquina.
    pub startup_output: Option<u16>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Updated { version: String },
    /// Trocada sem reiniciar (instalação na mão): `reiniciar_manual` no estado.
    UpdatedManual { version: String },
    UpToDate,
    Failed { voltou: bool, no_ar: bool, erro: String },
    Locked,
    /// O motor `git` do Python está rodando uma atualização nesta máquina (pid dele).
    OtherEngine { pid: u32 },
}

/// Versão e commit do pacote de onde um processo subiu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageId {
    pub version: String,
    pub commit: String,
    /// A tela web desta versão: a que este processo serve, mesmo com o `current` já em outra.
    pub dist_key: Option<String>,
}

/// O pacote cujo `bin/` guarda `exe`: o `manifest.json` da raiz acima. `None` fora de pacote.
pub fn package_of(exe: &Path) -> Option<PackageId> {
    let manifest = read_manifest(exe.parent()?.parent()?)?;
    Some(PackageId { version: manifest.version, commit: manifest.commit, dist_key: manifest.dist_key })
}

/// O pacote deste processo, lido uma vez (o servidor lê na subida, antes de qualquer troca).
pub fn running_package() -> Option<&'static PackageId> {
    static PACKAGE: std::sync::LazyLock<Option<PackageId>> =
        std::sync::LazyLock::new(|| package_of(&real(&std::env::current_exe().ok()?).ok()?));
    PACKAGE.as_ref()
}

/// Prova de vida da versão nova: o Rust responde com o contrato e o commit do manifesto novo (o
/// processo velho também responde HTTP); pid igual ao de antes do reinício também reprova.
pub(crate) async fn wait_rust<H: HealthProbe>(health: &H, manifest: &Manifest, before: Option<u32>, settings: &Settings) -> Result<(), String> {
    wait_version(health, manifest, before, settings, "a versão nova").await
}

/// `wait_rust` com o nome da versão esperada no motivo (a nova, na ida; a anterior, na volta).
async fn wait_version<H: HealthProbe>(health: &H, manifest: &Manifest, before: Option<u32>, settings: &Settings, what: &str) -> Result<(), String> {
    let (protocol, commit) = (manifest.protocol, manifest.commit.as_str());
    let deadline = Instant::now() + settings.health_deadline;
    loop {
        let why = match health.read().await {
            Reading::Rust { protocol: p, .. } if p != protocol => format!("o servidor responde com o contrato {p}, e {what} é {protocol}"),
            Reading::Rust { commit: None, .. } => "o servidor de pé não diz de que pacote subiu".to_owned(),
            Reading::Rust { commit: Some(c), .. } if c != commit => format!("o servidor de pé é do commit {c}, e {what} é {commit}"),
            Reading::Rust { pid: Some(pid), .. } if Some(pid) == before => format!("o processo na porta é o mesmo de antes (pid {pid})"),
            Reading::Rust { .. } => return Ok(()),
            Reading::Down(d) => format!("o servidor não respondeu ({d})"),
        };
        if Instant::now() >= deadline { return Err(format!("o servidor não voltou depois de reiniciar: {why}")); }
        tokio::time::sleep(settings.health_interval).await;
    }
}

/// Prova de vida sem versão (modo desenvolvedor, reinício avulso): o Rust, de qualquer versão,
/// responde com outro pid.
pub(crate) async fn wait_new<H: HealthProbe>(health: &H, before: Option<u32>, settings: &Settings) -> Result<(), String> {
    let deadline = Instant::now() + settings.health_deadline;
    loop {
        let why = match health.read().await {
            Reading::Rust { pid: Some(pid), .. } if Some(pid) == before => format!("o processo na porta é o mesmo de antes (pid {pid})"),
            Reading::Rust { .. } => return Ok(()),
            Reading::Down(d) => format!("o servidor não respondeu ({d})"),
        };
        if Instant::now() >= deadline { return Err(format!("o servidor não voltou depois de reiniciar: {why}")); }
        tokio::time::sleep(settings.health_interval).await;
    }
}

/// O `current` volta para `previous`, como estava antes da troca.
async fn put_back(layout: &Layout, previous: Option<PathBuf>) -> Result<(), UpdateError> {
    let layout = layout.clone();
    blocking(move || {
        match previous {
            Some(previous) => { swap(&layout, &previous)?; }
            // Não havia `current` antes desta rodada: voltar é tirá-lo.
            None => remove_link(&layout.current()).map_err(|e| UpdateError::Swap(format!("{} não saiu: {e}", layout.current().display())))?,
        }
        // Sobra com o `current` fora do alvo é apagada pela próxima recuperação.
        if let Err(e) = clear_unproven(&layout) {
            tracing::warn!(code = "update_unproven_not_cleared", path = %layout.unproven().display(), error = %e,
                "registro da troca sem prova ficou depois da volta");
        }
        Ok(())
    }).await
}

/// A troca vale: a recuperação não a desfaz mais. Registro que fica a desfaria na próxima rodada.
fn keep_swap(layout: &Layout, state: &StateFile) {
    if let Err(e) = clear_unproven(layout) {
        state.warn(&format!("apague {} à mão: com ele, a próxima atualização volta o current para a versão anterior ({e})", layout.unproven().display()));
    }
}

/// O processo que reprovou: quem responder depois da volta tem que ser outro. O da versão anterior
/// (`expected`) que nunca caiu (reinício recusado) já é a volta.
async fn failed_pid<H: HealthProbe>(health: &H, expected: Option<&Manifest>) -> Option<u32> {
    match health.read().await {
        Reading::Rust { pid, commit, .. } if commit.as_deref() != expected.map(|m| m.commit.as_str()) => pid,
        _ => None,
    }
}

/// A versão anterior de pé depois da volta, com outro pid que o de `failed_pid`.
async fn wait_previous<H: HealthProbe>(health: &H, expected: Option<&Manifest>, failed_pid: Option<u32>, settings: &Settings) -> Result<(), String> {
    match expected {
        Some(manifest) => wait_version(health, manifest, failed_pid, settings, "a versão anterior").await,
        // Sem pacote antes não há commit a conferir: basta o Rust com outro pid.
        None => wait_new(health, failed_pid, settings).await,
    }
}

/// Falha antes do reinício, com o `current` como estava: `no_ar` é medido.
async fn failed_before<H: HealthProbe>(state: &StateFile, health: &H, error: UpdateError) -> Outcome {
    tracing::warn!(code = error.code(), "update: {error}");
    let no_ar = !matches!(health.read().await, Reading::Down(_));
    let erro = error.to_string();
    state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": no_ar}));
    Outcome::Failed { voltou: false, no_ar, erro }
}

/// Falha depois da troca: o `current` volta para `previous`, o servidor reinicia e a resposta é
/// medida. `voltou` só com a volta inteira: troca, reinício e a versão anterior (`expected`)
/// respondendo, com outro pid, como a prova da ida.
/// Passo de máquina aplicado nesta rodada continua registrado: ele é compatível com a versão
/// anterior e não volta; o aviso diz quais.
#[allow(clippy::too_many_arguments)]
async fn roll_back<R: Restarter, H: HealthProbe>(job: &Job<'_>, state: &StateFile, previous: Option<PathBuf>, expected: Option<&Manifest>,
    applied_now: &[String], why: String, restarter: &R, health: &H) -> Outcome {
    tracing::warn!("update: voltando para a versão anterior: {why}");
    state.write(json!({"fase": "rodando", "texto": "Voltando para a versão anterior"}));
    if !applied_now.is_empty() {
        state.warn(&format!("passos de máquina aplicados nesta rodada continuam valendo na versão anterior: {}", applied_now.join(", ")));
    }
    let failed_pid = failed_pid(health, expected).await;
    let old = previous.clone();
    let back = Instant::now();
    let failure = match put_back(job.layout, previous).await {
        Err(e) => Some(format!("{why}; e não consegui voltar para a versão anterior: {e}")),
        Ok(()) => {
            let intent = stop_intent::RestartIntent::new(hangar_api::status::ExitKind::Update, "rollback")
                .between(None, expected.map(|m| m.version.as_str()));
            restarter.restart(intent).await.err().map(|e| format!("{why}; e a volta para a versão anterior também falhou: {e}"))
        }
    };
    // A máquina pode ficar fora do ar: o retrato da volta é o que diz por quê.
    let trail = startup_output::Trail::new(health);
    let detail = |lines: Vec<String>, old: Option<PathBuf>| async move {
        if let Some(port) = job.startup_output { log_failure_detail(lines, None, job.home, port, None, old).await; }
    };
    if let Some(erro) = failure {
        detail(vec![format!("volta: {erro}")], old).await;
        let no_ar = !matches!(health.read().await, Reading::Down(_));
        state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": no_ar}));
        return Outcome::Failed { voltou: false, no_ar, erro };
    }
    if let Err(unconfirmed) = wait_previous(&trail, expected, failed_pid, &job.settings).await {
        let mut lines = vec![format!("volta: a versão anterior não se confirmou em {}: {unconfirmed}", secs(back.elapsed()))];
        lines.extend(trail.lines("prova da volta"));
        detail(lines, old).await;
        let no_ar = !matches!(health.read().await, Reading::Down(_));
        let erro = format!("{why}; o current voltou para a versão anterior e o servidor reiniciou, mas a versão anterior não se confirmou no ar: {unconfirmed}");
        state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": no_ar}));
        return Outcome::Failed { voltou: false, no_ar, erro };
    }
    state.write(json!({"fase": "pronto", "ok": false, "erro": why, "voltou": true, "no_ar": true}));
    Outcome::Failed { voltou: true, no_ar: true, erro: why }
}

/// O que explica o reinício que não voltou, uma `update: saída` por linha, antes do `update: fim`:
/// `before` (etapas e prova), a saída da versão nova desde a marca e o retrato da máquina agora.
pub(crate) async fn log_failure_detail(before: Vec<String>, mark: Option<startup_output::Mark>, home: &Path, port: u16,
    new: Option<PathBuf>, old: Option<PathBuf>) {
    let home = home.to_path_buf();
    let lines = tokio::task::spawn_blocking(move || {
        let scrubber = crate::diag_report::Scrubber::new(Some(home.to_string_lossy().into_owned()), std::env::var("LOCALAPPDATA").ok());
        let mut lines = before;
        if let Some(mark) = mark { lines.extend(mark.lines(&scrubber).into_iter().map(|l| format!("log: {l}"))); }
        lines.extend(startup_output::snapshot(&home, port, new.as_deref(), old.as_deref()));
        lines.into_iter().map(|l| scrubber.scrub(&l).into_owned()).collect::<Vec<_>>()
    }).await.unwrap_or_else(|e| vec![format!("retrato da falha não montado: {e}")]);
    for line in lines { tracing::info!(line = %line, "update: saída"); }
}

fn secs(d: Duration) -> String { format!("{:.1} s", d.as_secs_f64()) }

/// Extração, passos, troca e poda bloqueiam por segundos ou minutos: fora dos fios do runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, UpdateError> + Send + 'static) -> Result<T, UpdateError> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|e| Err(UpdateError::Io(format!("tarefa da atualização morreu: {e}"))))
}

fn pid_alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some()
}

/// O motor `git` do Python atualizando esta máquina agora: `estado.json` em `rodando` com o dono
/// vivo (o `pid` do estado ou, antes de o motor gravá-lo, o do `rodando.lock`).
fn other_engine(state: &StateFile, state_dir: &Path) -> Option<u32> {
    let current = state.read();
    if current.get("fase").and_then(Value::as_str) != Some("rodando") { return None; }
    let pid = current.get("pid").and_then(Value::as_u64)
        .or_else(|| std::fs::read_to_string(state_dir.join("rodando.lock")).ok()?.trim().parse().ok())?;
    let pid = u32::try_from(pid).ok()?;
    pid_alive(pid).then_some(pid)
}

/// Quanto o executor tem para subir e gravar o pid dele (o `_JANELA_NASCIMENTO_S` do Python).
const BIRTH_WINDOW: chrono::TimeDelta = chrono::TimeDelta::seconds(120);

/// O estado para a tela, como o `estado_para_tela` do Python: `rodando` com o dono morto vira o
/// desfecho que ele não gravou (e fica gravado). Morreu no reinício que pediu com a versão nova
/// instalada e no ar (`running`): concluída. Quem chama é sempre o servidor de pé (rota, verificação
/// periódica, lançamento do executor), com `running` = a versão dele.
pub fn for_screen(state_dir: &Path, layout: &Layout, running: Option<&str>) -> StateSnapshot {
    let state = StateFile::new(state_dir);
    let current = state.read();
    if current.get("fase").and_then(Value::as_str) != Some("rodando") { return current; }
    let owner = current.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
    if owner.is_some_and(pid_alive) { return current; }
    let recent = current.get("ts").and_then(Value::as_str).and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .is_some_and(|ts| (chrono::Local::now().fixed_offset() - ts) < BIRTH_WINDOW);
    if owner.is_none() && recent { return current; }
    let target = current.get("versao_para").and_then(Value::as_str);
    let reached = current.get("etapa").and_then(Value::as_str) == Some("reiniciar") && target.is_some()
        && installed(layout).map(|m| m.version).as_deref() == target && running == target;
    if reached {
        tracing::info!(pid = ?owner, "update: concluída pelo servidor novo; o executor morreu no reinício que pediu");
        // A versão nova de pé é a prova que faltou.
        keep_swap(layout, &state);
        state.write(json!({"fase": "pronto", "ok": true, "texto": "Atualizado"}));
    } else {
        tracing::warn!(pid = ?owner, "update: atualização abandonada, o processo que a executava não existe mais");
        // A troca sem prova volta; com outra rodada segurando a trava, ela é que decide.
        let undone = match UpdateLock::try_acquire(layout) {
            Ok(_lock) => recover(layout).unwrap_or_else(|e| {
                tracing::warn!(code = "update_recover_failed", "update: a troca interrompida não voltou: {e}");
                state.warn(&format!("a troca interrompida não voltou para a versão anterior: {e}"));
                false
            }),
            Err(_) => false,
        };
        let disk = installed(layout).map(|m| m.version);
        let place = match (target, disk.as_deref()) {
            (Some(target), Some(disk)) if target == disk => format!("o current já aponta para a versão {target}, que sobe no próximo reinício"),
            (_, Some(disk)) => format!("o current segue na versão {disk}"),
            _ => "o current não aponta para uma versão em pacote".to_owned(),
        };
        let up = running.map_or_else(|| "de fora de pacote (versão desconhecida)".to_owned(), |v| format!("o da versão {v}"));
        // Só o servidor de pé chama esta função: `no_ar` é ele mesmo respondendo.
        let undone_text = if undone { "a troca sem prova foi desfeita, " } else { "" };
        // Desfeita com a versão nova já de pé (reinício no meio dos passos): a anterior sobe no próximo reinício.
        let restart = undone && running.is_some() && running != disk.as_deref();
        state.write(json!({"fase": "pronto", "ok": false, "voltou": false, "no_ar": true, "reiniciar_manual": restart,
            "erro": format!("a atualização foi interrompida, o processo que a executava não existe mais; {undone_text}{place}, e o servidor de pé é {up}")}));
    }
    state.read()
}

/// Uma atualização inteira: trava → índice → pacote → troca → passos → reinício → prova → poda.
///
/// Roda no runtime próprio do executor destacado (processo só disso, nunca o do servidor); mesmo
/// assim o trabalho de bloqueio vai para `spawn_blocking`, para não segurar fio do runtime.
pub async fn run<R: Restarter, H: HealthProbe>(job: &Job<'_>, steps: std::sync::Arc<dyn StepRunner + Send + Sync>, restarter: &R, health: &H,
    in_use: std::sync::Arc<dyn InUse + Send + Sync>) -> Outcome {
    let state = StateFile::new(job.state_dir);
    let _lock = match UpdateLock::try_acquire(job.layout) {
        Ok(lock) => lock,
        // A recusa não toca no estado: ele é da rodada que está de pé.
        Err(UpdateError::Locked) => return Outcome::Locked,
        Err(e) => return failed_before(&state, health, e).await,
    };
    // O estado é o mesmo arquivo do motor `git`: com ele rodando, nada aqui é tocado.
    if let Some(pid) = other_engine(&state, job.state_dir) {
        tracing::warn!(pid, "update: o motor git do Python está atualizando esta máquina; recusado");
        return Outcome::OtherEngine { pid };
    }
    match recover(job.layout) {
        Ok(true) => {
            tracing::warn!("update: current recuperado de uma troca interrompida");
            crate::diag::record_blocking(job.diary_dir.as_deref(), &[json!({"evento": "rust.update_swap_recovered", "nivel": "aviso",
                "detalhe": format!("{} voltou a apontar para a versão de antes da troca interrompida", job.layout.current().display())})], "update");
        }
        Ok(false) => {}
        Err(e) => return failed_before(&state, health, UpdateError::Swap(format!("troca interrompida não recuperada: {e}"))).await,
    }
    if let Some(why) = broken_manifest(job.layout) {
        return failed_before(&state, health, UpdateError::Io(why)).await;
    }
    let before = installed(job.layout);
    if let Err(error)=state.try_write(json!({"fase": "rodando", "ok": null, "erro": null, "resgate": null, "voltou": null, "no_ar": null,
        "commit_de": before.as_ref().map_or("", |m| m.commit.as_str()), "commit_para": null,
        "versao_de": before.as_ref().map(|m| m.version.clone()), "versao_para": null,
        "pid": std::process::id(), "backend_pid": null, "reiniciar_manual": false, "shell_mudou": false,
        "waiting_for_server_build": false, "request_id": null, "log": [], "avisos": [], "passos_invalidos": []})) {return failed_before(&state,health,UpdateError::Io(error)).await;}
    for note in &job.notes { state.warn(note); }

    state.step("baixar");
    let downloading = Instant::now();
    // Trocar de canal instala o publicado no outro, mesmo com número menor que o instalado.
    let installed_version = before.as_ref().filter(|m| m.channel == job.release.channel).and_then(|m| m.version.parse().ok());
    let entry = match check(job.client, job.release, job.platform, installed_version).await {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            if let Err(error)=state.try_write(json!({"fase": "pronto", "ok": true, "texto": "Já está na versão mais nova"})) {return failed_before(&state,health,UpdateError::Io(error)).await;}
            return Outcome::UpToDate;
        }
        Err(e) => return failed_before(&state, health, e).await,
    };
    if let Err(error)=state.try_write(json!({"commit_para": entry.commit, "versao_para": entry.version})) {return failed_before(&state,health,UpdateError::Io(error)).await;}
    let dir = match prepare(job.client, job.release, &entry, job.layout, job.settings.download_deadline).await {
        Ok(dir) => dir,
        Err(e) => return failed_before(&state, health, e).await,
    };
    let Some(manifest) = read_manifest(&dir) else {
        return failed_before(&state, health, UpdateError::Io(format!("{} sem manifesto legível", dir.display()))).await;
    };
    // A tela é opcional: falha deixa a da chave anterior servida, e a atualização segue.
    if let (Some(ui), Some(key)) = (job.ui, manifest.dist_key.as_deref()) {
        // Escolha ilegível: a tela fica ligada (o padrão), e a leitura que falhou vai ao diário.
        let web_ui = crate::ui::enabled(job.home).unwrap_or_else(|e| {
            tracing::warn!(code = "update_web_ui_unreadable", "update: escolha da tela web ilegível, segue ligada: {e}");
            crate::diag::record_blocking(job.diary_dir.as_deref(), &[json!({"evento": "rust.update_web_ui_unreadable", "nivel": "aviso",
                "detalhe": format!("~/.hangar/web-ui.json: {e}; a tela segue ligada")})], "update");
            true
        });
        if web_ui && let Err(e) = ui.fetch(key, &job.layout.hangar.join("ui")).await {
            state.warn(&format!("a tela web não foi atualizada e segue a anterior: {e}"));
        }
    }

    state.step("trocar");
    let swapping = Instant::now();
    let pid_before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let previous = {
        let (layout, dir) = (job.layout.clone(), dir.clone());
        match blocking(move || note_unproven(&layout, &dir).and_then(|()| swap(&layout, &dir).inspect_err(|_| { let _ = recover(&layout); }))).await {
            Ok(previous) => previous,
            Err(e) => return failed_before(&state, health, e).await,
        }
    };

    // Depois da troca: o instalador que um passo chama só grava links pelo `current` quando ele já é
    // a versão dele; antes, os prenderia à pasta da versão, que a poda apaga.
    state.step("passos");
    let stepping = Instant::now();
    let steps_mark = job.startup_output.map(|_| startup_output::Mark::take(false));
    let applied_before = self::steps::applied(job.state_dir).unwrap_or_default();
    let applied_now = {
        let (root, home, state_dir, log) = (dir.clone(), job.home.to_path_buf(), job.state_dir.to_path_buf(), state.clone());
        match blocking(move || apply_machine_steps(&root, &home, &state_dir, &*steps, &log)).await {
            Ok(ids) => ids,
            Err(e) => {
                if let Some(port) = job.startup_output {
                    let mut lines = vec![format!("etapas: baixar {} · trocar {} · passos {}; pid antes {}",
                        secs(swapping - downloading), secs(stepping - swapping), secs(stepping.elapsed()),
                        pid_before.map_or("?".into(), |p| p.to_string())), format!("passo: {e}")];
                    // shortcut: só a cauda do registro da rodada; o instalador escreve centenas de linhas e o fim é o que explica.
                    let log = state.read().get("log").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
                    lines.extend(log[log.len().saturating_sub(80)..].iter().map(|l| format!("saída do passo: {l}")));
                    log_failure_detail(lines, steps_mark, job.home, port, Some(dir.clone()), previous.clone()).await;
                }
                // Passo que reiniciou o servidor (o `install.ps1 -Update` reinicia) deixou a versão nova de pé:
                // só devolver o `current` deixaria a nova rodando com o `current` na anterior.
                // Mesmo commit antes e depois (troca de canal) não distingue versão: aí só o pid diz.
                let new_commit = before.as_ref().is_none_or(|b| b.commit != manifest.commit);
                let restarted = match health.read().await {
                    Reading::Rust { pid, commit, .. } =>
                        (new_commit && commit.as_deref() == Some(manifest.commit.as_str())) || (pid_before.is_some() && pid != pid_before),
                    // Saúde fora com o processo de antes vivo é tropeço da leitura, não reinício.
                    Reading::Down(_) => pid_before.is_some_and(|pid| !pid_alive(pid)),
                };
                if restarted && !restarter.manual() {
                    let applied_now: Vec<String> = self::steps::applied(job.state_dir).unwrap_or_default().difference(&applied_before).cloned().collect();
                    return roll_back(job, &state, previous, before.as_ref(), &applied_now, e.to_string(), restarter, health).await;
                }
                let e = match put_back(job.layout, previous).await {
                    Ok(()) => e,
                    Err(back) => UpdateError::Step(format!("{e}; e o current não voltou para a versão anterior: {back}")),
                };
                return failed_before(&state, health, e).await;
            }
        }
    };

    // O lançador estável é cópia do `hangar-server`: renovado a cada versão, ele sabe subir a nova.
    // Falha é aviso: o de antes ainda sobe a versão anterior, e a prova devolve o `current` se a nova não subir.
    {
        let (home, source) = (job.home.to_path_buf(), dir.clone());
        if let Err(e) = blocking(move || migrate::refresh_launcher(&home, &source, cfg!(windows)).map_err(UpdateError::Io)).await {
            state.warn(&format!("o lançador do serviço não foi renovado: {e}"));
        }
    }

    // Na mão não há reinício nem prova: o servidor de antes segue no ar até alguém reiniciá-lo.
    if restarter.manual() {
        keep_swap(job.layout, &state);
        if let Err(error)=state.try_write(json!({"fase": "pronto", "ok": true, "texto": "Atualizado", "reiniciar_manual": true})) {return failed_before(&state,health,UpdateError::Io(error)).await;}
        return Outcome::UpdatedManual { version: manifest.version };
    }

    state.step("reiniciar");
    let restarting = Instant::now();
    let mark = job.startup_output.map(|_| startup_output::Mark::take(false));
    let trail = startup_output::Trail::new(health);
    let intent = stop_intent::RestartIntent::new(hangar_api::status::ExitKind::Update, "package")
        .between(before.as_ref().map(|m| m.version.as_str()), Some(manifest.version.as_str()));
    let restarted = restarter.restart(intent).await;
    let restart_took = restarting.elapsed();
    let proved = match restarted {
        Err(e) => Err(format!("o servidor não reiniciou: {e}")),
        Ok(()) => wait_rust(&trail, &manifest, pid_before, &job.settings).await,
    };
    if let Err(why) = proved {
        if let Some(port) = job.startup_output {
            let mut lines = vec![format!("etapas: baixar {} · trocar {} · passos {} · comando de reinício {} · espera da versão nova {} (prazo {}); pid antes {}",
                secs(swapping - downloading), secs(stepping - swapping), secs(restarting - stepping), secs(restart_took),
                secs(restarting.elapsed().saturating_sub(restart_took)), secs(job.settings.health_deadline),
                pid_before.map_or("?".into(), |p| p.to_string()))];
            lines.extend(trail.lines("prova"));
            log_failure_detail(lines, mark, job.home, port, Some(dir.clone()), previous.clone()).await;
        }
        return roll_back(job, &state, previous, before.as_ref(), &applied_now, why, restarter, health).await;
    }
    keep_swap(job.layout, &state);

    let layout = job.layout.clone();
    let pruned = blocking(move || Ok(prune(&layout, previous.as_deref(), &*in_use))).await;
    for failure in pruned.map_or_else(|e| vec![e.to_string()], |p| p.failed) {
        state.warn(&failure);
    }
    if let Err(error)=state.try_write(json!({"fase": "pronto", "ok": true, "texto": "Atualizado"})) {return failed_before(&state,health,UpdateError::Io(error)).await;}
    Outcome::Updated { version: manifest.version }
}
