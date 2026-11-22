//! O trabalho de instalação: login (com o código de 2FA vindo da tela), limite de apps, download do
//! IPA provado, assinatura, instalação, pareamento e a conferência no aparelho. Um por aparelho.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use super::apple::{AppleSigner, AskCerts, AskCode, CertInfo, CodeReply, Login, SignRequest, anisette_servers};
use super::device::create_private_dir;
use super::{DeviceError, DeviceLink, lock};

/// O SideStore que foi provado no iPad, numa cópia nossa: o asset `nightly` do GitHub é
/// sobrescrito a cada build. Atualizar: baixar o nightly novo, provar no aparelho, subir o IPA na
/// release `sidestore-pinned` deste repositório e trocar aqui versão, sha256 e tamanho juntos.
/// Versão: `0.7.0-20261016.1506+d97ec133`.
/// SideStore é AGPL-3.0 (fonte no commit `d97ec133` de github.com/SideStore/SideStore).
pub const SIDESTORE_IPA: Pinned = Pinned {
    url: "https://github.com/jeffer1312/hangar/releases/download/sidestore-pinned/SideStore.ipa",
    sha256: "b6855f5a055e3068e136f2bafb97d6da1cbea188a7d1778dd26d3d8d8a4caad2",
    size: 31_137_643,
};

/// Fonte de apps que o SideStore abre já com o Hangar.
pub const SOURCE_URL: &str = "https://github.com/jeffer1312/hangar/releases/download/mobile-latest/sidestore-source.json";

const SIDESTORE_BUNDLE: &str = "com.SideStore.SideStore";
/// A Apple ID gratuita aceita três apps ativos por aparelho.
const FREE_APP_LIMIT: u32 = 3;
const CODE_WAIT: Duration = Duration::from_secs(600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
// Global do processo, não do `Jobs`: o temp de extração do isideload é do processo inteiro.
static SIGNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug)]
pub struct Pinned {
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const WRONG_CODE: &str = "device_install_wrong_code";
pub const CODE_TIMEOUT: &str = "device_install_code_timeout";
pub const APP_LIMIT: &str = "device_install_app_limit";
pub const APPID_LIMIT: &str = "device_install_appid_limit";
pub const CERT_LIMIT: &str = "device_install_cert_limit";
pub const DISCONNECTED: &str = "device_install_disconnected";
pub const LOCKED: &str = "device_install_locked";
pub const SERVICE_MISSING: &str = "device_service_missing";
pub const NOT_TRUSTED: &str = "device_not_trusted";
pub const DEVICE_ERROR: &str = "device_error";
pub const LOGIN_REFUSED: &str = "device_install_login_refused";
pub const APPLE_BUSY: &str = "device_install_apple_busy";
pub const ANISETTE_UNREACHABLE: &str = "device_install_anisette_unreachable";
pub const DOWNLOAD_FAILED: &str = "device_install_download_failed";
pub const DOWNLOAD_MISMATCH: &str = "device_install_download_mismatch";
pub const SIGN_FAILED: &str = "device_install_sign_failed";
pub const NOT_INSTALLED: &str = "device_install_not_installed";
pub const BUSY: &str = "device_install_busy";
pub const CANCELLED: &str = "device_install_cancelled";
pub const FORBIDDEN: &str = "device_install_forbidden";
pub const UNKNOWN_JOB: &str = "device_install_unknown_job";
pub const NOT_WAITING: &str = "device_install_not_waiting";
pub const BAD_REQUEST: &str = "device_install_bad_request";

/// Erro que a tela explica: código fixo e, quando ajuda, o detalhe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub detail: Option<String>,
}

impl Failure {
    pub fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }
    pub fn with(code: &'static str, detail: impl Into<String>) -> Self {
        Self { code, detail: Some(detail.into()) }
    }
}

impl From<DeviceError> for Failure {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::ServiceMissing => Self::new(SERVICE_MISSING),
            DeviceError::NotTrusted => Self::new(NOT_TRUSTED),
            DeviceError::Locked => Self::new(LOCKED),
            DeviceError::Disconnected => Self::new(DISCONNECTED),
            DeviceError::Other(detail) => Self::with(DEVICE_ERROR, detail),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Login,
    AwaitingCode,
    CheckingLimit,
    Downloading,
    Signing,
    AwaitingCertChoice,
    Installing,
    Pairing,
    Verifying,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub stage: Stage,
    pub error: Option<Failure>,
    pub source_url: &'static str,
    /// Só em `awaiting_cert_choice`: os certificados que lotam a conta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certs: Option<Vec<CertInfo>>,
}

struct Job {
    udid: String,
    progress: Mutex<Progress>,
    /// Quem espera o código: só existe enquanto o trabalho está em `awaiting_code`.
    code: Mutex<Option<oneshot::Sender<CodeReply>>>,
    /// Quem espera a escolha dos certificados: só em `awaiting_cert_choice`.
    cert_choice: Mutex<Option<oneshot::Sender<Option<Vec<String>>>>>,
    timed_out: std::sync::atomic::AtomicBool,
    cancelled: std::sync::atomic::AtomicBool,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Job {
    fn set(&self, stage: Stage, error: Option<Failure>) {
        let mut p = lock(&self.progress);
        p.stage = stage;
        p.error = error;
        p.certs = None;
    }
    fn running(&self) -> bool {
        !matches!(lock(&self.progress).stage, Stage::Done | Stage::Failed)
    }
    /// Passa a instalar só se ninguém cancelou: o abort pedido no meio da assinatura ainda deixaria
    /// a tarefa chegar aqui antes do próximo `await`.
    fn enter_install(&self) -> Result<(), Failure> {
        let mut p = lock(&self.progress);
        if self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Failure::new(CANCELLED));
        }
        (p.stage, p.error, p.certs) = (Stage::Installing, None, None);
        Ok(())
    }
}

/// Pedido de início; a senha nunca sai daqui a não ser para o login.
#[derive(serde::Deserialize)]
pub struct Start {
    pub udid: String,
    pub apple_id: String,
    pub password: Zeroizing<String>,
}

pub struct DeviceInstall {
    pub link: Arc<dyn DeviceLink>,
    signer: Arc<dyn AppleSigner>,
    /// `~/.hangar/devices`: estado do isideload e downloads temporários (0700).
    dir: PathBuf,
    pinned: Pinned,
    config: Arc<dyn Fn() -> Value + Send + Sync>,
    code_wait: Duration,
    http: reqwest::Client,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl DeviceInstall {
    pub fn new(link: Arc<dyn DeviceLink>, signer: Arc<dyn AppleSigner>, hangar_dir: &Path) -> Self {
        Self {
            link,
            signer,
            dir: hangar_dir.join("devices"),
            pinned: SIDESTORE_IPA,
            config: Arc::new(|| Value::Null),
            code_wait: CODE_WAIT,
            http: crate::http_client().timeout(DOWNLOAD_TIMEOUT).build().expect("cliente http da instalação nos aparelhos"),
            jobs: Mutex::default(),
        }
    }
    /// O `runtime-config.json` lido a cada trabalho (a chave `device_install_anisette`).
    pub fn with_config(mut self, config: Arc<dyn Fn() -> Value + Send + Sync>) -> Self {
        self.config = config;
        self
    }
    pub fn with_pinned(mut self, pinned: Pinned) -> Self {
        self.pinned = pinned;
        self
    }
    pub fn with_code_wait(mut self, wait: Duration) -> Self {
        self.code_wait = wait;
        self
    }

    /// Começa o trabalho; outro ainda rodando no mesmo aparelho é `device_install_busy`.
    pub fn start(self: &Arc<Self>, start: Start) -> Result<String, Failure> {
        let id = crate::mods::state::random_hex(16);
        let job = Arc::new(Job {
            udid: start.udid.clone(),
            progress: Mutex::new(Progress { stage: Stage::Login, error: None, source_url: SOURCE_URL, certs: None }),
            code: Mutex::default(),
            cert_choice: Mutex::default(),
            timed_out: Default::default(),
            cancelled: Default::default(),
            task: Mutex::default(),
        });
        {
            let mut jobs = lock(&self.jobs);
            if jobs.values().any(|j| j.udid == start.udid && j.running()) {
                return Err(Failure::new(BUSY));
            }
            // ponytail: só o último trabalho de cada aparelho fica para consulta.
            jobs.retain(|_, j| j.udid != start.udid);
            jobs.insert(id.clone(), job.clone());
        }
        let svc = self.clone();
        tokio::spawn(async move {
            // Tarefa interna: um pânico (isideload) vira `failed` em vez de deixar o aparelho ocupado.
            let inner = job.clone();
            let handle = tokio::spawn(async move { svc.run(&inner, start).await });
            *lock(&job.task) = Some(handle.abort_handle());
            // Cancelado antes de a tarefa ficar guardada: `cancel` não tinha o que abortar.
            if job.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                handle.abort();
            }
            let outcome = handle.await.unwrap_or_else(|e| {
                if e.is_cancelled() { Err(Failure::new(CANCELLED)) } else { Err(Failure::with(DEVICE_ERROR, panic_message(e))) }
            });
            match outcome {
                Ok(()) => job.set(Stage::Done, None),
                Err(f) => {
                    // O detalhe vem dos erros do aparelho/Apple; a senha nunca entra num `Failure`.
                    tracing::warn!(code = f.code, detail = f.detail.as_deref().unwrap_or(""), "device install failed");
                    job.set(Stage::Failed, Some(f));
                }
            }
            lock(&job.code).take();
            lock(&job.cert_choice).take();
        });
        Ok(id)
    }

    pub fn progress(&self, id: &str) -> Option<Progress> {
        lock(&self.jobs).get(id).map(|j| lock(&j.progress).clone())
    }

    /// Para o trabalho do aparelho antes da instalação; depois dela, parar deixaria o SideStore
    /// sem pareamento, então responde `device_install_busy`.
    pub fn cancel(&self, udid: &str) -> Result<(), Failure> {
        let job = lock(&self.jobs).values().find(|j| j.udid == udid && j.running()).cloned().ok_or_else(|| Failure::new(NOT_WAITING))?;
        {
            // Mesma trava do `enter_install`: ou o cancelamento vem antes e a instalação não começa,
            // ou ela já começou e o pedido é recusado.
            let progress = lock(&job.progress);
            if matches!(progress.stage, Stage::Installing | Stage::Pairing | Stage::Verifying) {
                return Err(Failure::new(BUSY));
            }
            job.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(task) = lock(&job.task).as_ref() {
            task.abort();
        }
        Ok(())
    }

    /// Entrega o código (ou o pedido de reenvio) ao login que está esperando.
    pub fn answer(&self, id: &str, reply: CodeReply) -> Result<(), Failure> {
        let job = lock(&self.jobs).get(id).cloned().ok_or_else(|| Failure::new(UNKNOWN_JOB))?;
        let tx = lock(&job.code).take().ok_or_else(|| Failure::new(NOT_WAITING))?;
        job.set(Stage::Login, None);
        tx.send(reply).map_err(|_| Failure::new(NOT_WAITING))
    }

    /// `Some(seriais)` revoga esses certificados e segue a assinatura; `None` desiste do trabalho.
    pub fn choose_certs(&self, id: &str, revoke: Option<Vec<String>>) -> Result<(), Failure> {
        let job = lock(&self.jobs).get(id).cloned().ok_or_else(|| Failure::new(UNKNOWN_JOB))?;
        let mut waiting = lock(&job.cert_choice);
        if waiting.is_none() {
            return Err(Failure::new(NOT_WAITING));
        }
        // Só se revoga o que a tela mostrou.
        let listed = lock(&job.progress).certs.clone().unwrap_or_default();
        if let Some(serials) = &revoke && (serials.is_empty() || !serials.iter().all(|s| listed.iter().any(|c| &c.serial == s))) {
            return Err(Failure::new(BAD_REQUEST));
        }
        let tx = waiting.take().ok_or_else(|| Failure::new(NOT_WAITING))?;
        drop(waiting);
        job.set(Stage::Signing, None);
        tx.send(revoke).map_err(|_| Failure::new(NOT_WAITING))
    }

    async fn run(&self, job: &Arc<Job>, start: Start) -> Result<(), Failure> {
        let Start { udid, apple_id, password } = start;
        let device = self.link.list().await?.into_iter().find(|d| d.udid == udid).ok_or_else(|| Failure::new(DISCONNECTED))?;
        if !device.trusted {
            return Err(Failure::new(NOT_TRUSTED));
        }
        // Problema no aparelho aparece antes de a senha ir para a Apple.
        let pairing = self.link.pairing_file(&udid).await?;
        create_private_dir(&self.dir).map_err(|e| Failure::with(DEVICE_ERROR, e.to_string()))?;

        let login = Login { apple_id, password, anisette: anisette_servers(&(self.config)()), storage: self.dir.join("apple") };
        let mut session = self.signer.login(login, self.ask_code(job), self.ask_certs(job)).await.map_err(|f| {
            if job.timed_out.load(std::sync::atomic::Ordering::Relaxed) { Failure::new(CODE_TIMEOUT) } else { f }
        })?;

        job.set(Stage::CheckingLimit, None);
        let bundle = format!("{SIDESTORE_BUNDLE}.{}", session.team_id().await?);
        // Reinstalar o SideStore não ocupa vaga nova.
        if self.link.dev_apps(&udid).await? >= FREE_APP_LIMIT && !self.link.is_installed(&udid, &bundle).await? {
            return Err(Failure::new(APP_LIMIT));
        }

        job.set(Stage::Downloading, None);
        let tmp = tempfile::tempdir_in(&self.dir).map_err(|e| Failure::with(DOWNLOAD_FAILED, e.to_string()))?;
        let ipa = download(&self.http, &self.pinned, tmp.path()).await?;

        // O isideload extrai sempre no mesmo lugar do temp: um aparelho assina e instala por vez.
        // ponytail: login, anisette e Team ID acima também escrevem em `devices/apple` e rodam fora
        // da trava; duas instalações ao mesmo tempo podem se atropelar ali. Trava por conta se importar.
        {
            let _one = SIGNING.lock().await;
            job.set(Stage::Signing, None);
            let signed = session.sign(SignRequest { ipa: &ipa, udid: &udid, device_name: &device.name, pairing: &pairing }).await?;
            drop(session);
            job.enter_install()?;
            self.link.install_sidestore(&udid, &signed.app).await?;
        }

        // Antes de a pessoa abrir o SideStore: aberto, ele guardaria as preferências antigas.
        job.set(Stage::Pairing, None);
        self.link.write_pairing(&udid, &bundle).await?;
        self.link.write_sidestore_prefs(&udid, &bundle).await?;

        job.set(Stage::Verifying, None);
        if !self.link.is_installed(&udid, &bundle).await? {
            return Err(Failure::with(NOT_INSTALLED, bundle));
        }
        Ok(())
    }

    /// Ponte do 2FA: o trabalho fica em `awaiting_code` até a rota `/code` responder ou o prazo vencer.
    fn ask_code(&self, job: &Arc<Job>) -> AskCode {
        let (job, wait) = (job.clone(), self.code_wait);
        Arc::new(move |wrong: bool| {
            let job = job.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                *lock(&job.code) = Some(tx);
                job.set(Stage::AwaitingCode, wrong.then(|| Failure::new(WRONG_CODE)));
                match tokio::time::timeout(wait, rx).await {
                    Ok(Ok(reply)) => Some(reply),
                    _ => {
                        lock(&job.code).take();
                        job.timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
                        None
                    }
                }
            })
        })
    }

    /// Ponte do limite de certificados: `awaiting_cert_choice` até a rota `/certs` responder ou o
    /// prazo vencer. ponytail: espera com a trava de assinatura na mão; outro aparelho fica na fila.
    fn ask_certs(&self, job: &Arc<Job>) -> AskCerts {
        let (job, wait) = (job.clone(), self.code_wait);
        Arc::new(move |certs: Vec<CertInfo>| {
            let job = job.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                job.set(Stage::AwaitingCertChoice, None);
                lock(&job.progress).certs = Some(certs);
                *lock(&job.cert_choice) = Some(tx);
                let choice = tokio::time::timeout(wait, rx).await.ok().and_then(Result::ok).flatten();
                lock(&job.cert_choice).take();
                choice
            })
        })
    }
}

/// Baixa o IPA e confere tamanho e sha256 antes de qualquer assinatura.
async fn download(http: &reqwest::Client, pinned: &Pinned, dir: &Path) -> Result<PathBuf, Failure> {
    let failed = |e: reqwest::Error| Failure::with(DOWNLOAD_FAILED, e.to_string());
    let mut resp = http.get(pinned.url).send().await.and_then(reqwest::Response::error_for_status).map_err(failed)?;
    let mut bytes = Vec::with_capacity(pinned.size as usize);
    while let Some(chunk) = resp.chunk().await.map_err(failed)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > pinned.size {
            break;
        }
    }
    let sha = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    if bytes.len() as u64 != pinned.size || sha != pinned.sha256 {
        return Err(Failure::with(DOWNLOAD_MISMATCH, format!("{} bytes sha256 {sha}", bytes.len())));
    }
    let path = dir.join("SideStore.ipa");
    std::fs::write(&path, &bytes).map_err(|e| Failure::with(DOWNLOAD_FAILED, e.to_string()))?;
    Ok(path)
}

fn panic_message(e: tokio::task::JoinError) -> String {
    let Ok(payload) = e.try_into_panic() else { return "cancelled".into() };
    match payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()) {
        Some(msg) => format!("panic: {msg}"),
        None => "panic".into(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
