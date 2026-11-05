//! Login na Apple ID (com o 2FA) e assinatura do SideStore pela conta da pessoa.
//! A implementação real usa o isideload, portado do iloader (MIT, github.com/nab138/iloader).
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use super::lock;
use super::job::{
    ANISETTE_UNREACHABLE, APPID_LIMIT, APPLE_BUSY, CERT_LIMIT, DISCONNECTED, Failure, LOGIN_REFUSED, SIGN_FAILED,
};

/// Ordem provada: os servidores v3 comuns levam `-80045` da Apple; o `mac-ani` passa.
pub const ANISETTE: [&str; 3] = ["https://mac-ani.jkcoxson.com", "https://ani.sidestore.app", "https://ani.jkcoxson.com"];

pub enum CodeReply {
    Submit(String),
    Resend,
}

/// Pede o código à tela. `true` = o último foi recusado. `None` = ninguém respondeu no prazo.
pub type AskCode = Arc<dyn Fn(bool) -> BoxFuture<'static, Option<CodeReply>> + Send + Sync>;

/// Certificado de desenvolvimento que já ocupa a vaga da conta; `serial` é o que se revoga.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CertInfo {
    pub name: String,
    pub machine_name: String,
    pub serial: String,
}

/// Mostra os certificados à tela. `Some(seriais)` = revogar esses e seguir; `None` = desistiu ou o prazo venceu.
pub type AskCerts = Arc<dyn Fn(Vec<CertInfo>) -> BoxFuture<'static, Option<Vec<String>>> + Send + Sync>;

/// Sem `Debug`: leva a senha.
pub struct Login {
    pub apple_id: String,
    pub password: Zeroizing<String>,
    pub anisette: Vec<String>,
    /// Estado do anisette e chave do certificado de desenvolvimento; nunca a senha.
    pub storage: PathBuf,
}

pub struct SignRequest<'a> {
    pub ipa: &'a Path,
    pub udid: &'a str,
    pub device_name: &'a str,
    /// Vai cifrado dentro do bundle do SideStore.
    pub pairing: &'a [u8],
}

/// Pasta onde o IPA foi extraído e assinado, com a chave do certificado dentro: apagada quando sai de
/// cena, inclusive por erro ou pânico no meio da assinatura.
pub struct Scratch(pub PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// O `.app` assinado vive dentro de `scratch`: instalar antes de soltar.
pub struct Signed {
    pub app: PathBuf,
    pub scratch: Scratch,
}

pub trait AppleSigner: Send + Sync {
    fn login(&self, login: Login, ask: AskCode, certs: AskCerts) -> BoxFuture<'_, Result<Box<dyn AppleSession>, Failure>>;
}

pub trait AppleSession: Send {
    fn team_id(&mut self) -> BoxFuture<'_, Result<String, Failure>>;
    /// Registra o aparelho e devolve o `.app` assinado.
    fn sign<'a>(&'a mut self, req: SignRequest<'a>) -> BoxFuture<'a, Result<Signed, Failure>>;
}

/// `device_install_anisette` do runtime-config na frente da lista provada.
pub fn anisette_servers(config: &Value) -> Vec<String> {
    let mine = config["device_install_anisette"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let mut out: Vec<String> = mine.into_iter().map(str::to_owned).collect();
    out.extend(ANISETTE.iter().filter(|s| Some(**s) != mine).map(|s| s.to_string()));
    out
}

pub enum Attempt {
    /// O servidor de anisette não respondeu: o próximo pode.
    Down(String),
    /// Qualquer outro erro: senha errada não vai a um segundo servidor.
    Fatal(Failure),
}

/// Tenta cada servidor até um aceitar; só falha de anisette passa ao próximo.
pub async fn with_anisette<T, F, Fut>(servers: &[String], mut attempt: F) -> Result<T, Failure>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<T, Attempt>>,
{
    let mut last = String::new();
    for url in servers {
        match attempt(url.clone()).await {
            Ok(t) => return Ok(t),
            Err(Attempt::Down(e)) => {
                tracing::warn!(%url, "anisette unavailable: {e}");
                last = format!("{url}: {e}");
            }
            Err(Attempt::Fatal(f)) => return Err(f),
        }
    }
    Err(Failure::with(ANISETTE_UNREACHABLE, last))
}

/// Assinatura de mentira para os testes das rotas: o código certo é `code`, o Team ID é `team_id`.
#[derive(Default)]
pub struct FakeSigner {
    pub code: String,
    pub team_id: String,
    /// Servidores de anisette fora do ar.
    pub down: Vec<String>,
    pub sign_error: Option<Failure>,
    /// Assinatura que entra em pânico depois de extrair, como um erro inesperado do isideload.
    pub sign_panics: bool,
    /// Quanto a assinatura demora, e quantas chegaram a rodar juntas.
    pub sign_delay: std::time::Duration,
    pub signing: Arc<(std::sync::atomic::AtomicUsize, std::sync::atomic::AtomicUsize)>,
    pub tried: Mutex<Vec<String>>,
    /// Servidor que recebeu a senha, uma entrada por envio.
    pub logins: Mutex<Vec<String>>,
    pub resends: Mutex<u32>,
    pub signed: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    /// Certificados que já lotam a conta: a assinatura para e pergunta quais revogar.
    pub cert_limit: Vec<CertInfo>,
    pub revoked: Arc<Mutex<Vec<String>>>,
}

impl AppleSigner for FakeSigner {
    fn login(&self, login: Login, ask: AskCode, certs: AskCerts) -> BoxFuture<'_, Result<Box<dyn AppleSession>, Failure>> {
        Box::pin(async move {
            let url = with_anisette(&login.anisette, |url| {
                lock(&self.tried).push(url.clone());
                let down = self.down.contains(&url);
                async move { if down { Err(Attempt::Down("fake".into())) } else { Ok(url) } }
            })
            .await?;
            lock(&self.logins).push(url);
            drop(login);
            let mut wrong = false;
            loop {
                match ask(wrong).await {
                    None => return Err(Failure::with(LOGIN_REFUSED, "no 2FA code")),
                    Some(CodeReply::Resend) => {
                        *lock(&self.resends) += 1;
                        wrong = false;
                    }
                    Some(CodeReply::Submit(c)) if c == self.code => break,
                    Some(CodeReply::Submit(_)) => wrong = true,
                }
            }
            let session = FakeSession {
                team_id: self.team_id.clone(),
                error: self.sign_error.clone(),
                panics: self.sign_panics,
                delay: self.sign_delay,
                signing: self.signing.clone(),
                signed: self.signed.clone(),
                cert_limit: self.cert_limit.clone(),
                revoked: self.revoked.clone(),
                ask_certs: certs,
            };
            Ok(Box::new(session) as Box<dyn AppleSession>)
        })
    }
}

struct FakeSession {
    team_id: String,
    error: Option<Failure>,
    panics: bool,
    delay: std::time::Duration,
    signing: Arc<(std::sync::atomic::AtomicUsize, std::sync::atomic::AtomicUsize)>,
    signed: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    cert_limit: Vec<CertInfo>,
    revoked: Arc<Mutex<Vec<String>>>,
    ask_certs: AskCerts,
}

/// Onde o fake extrai: `<pasta acima do IPA>/<nome do IPA>_extracted`, como o isideload faz no temp.
pub fn fake_extract_dir(ipa: &Path) -> PathBuf {
    let name = ipa.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ipa.parent().and_then(Path::parent).unwrap_or(Path::new(".")).join(format!("{name}_extracted"))
}

impl AppleSession for FakeSession {
    fn team_id(&mut self) -> BoxFuture<'_, Result<String, Failure>> {
        let id = self.team_id.clone();
        Box::pin(async move { Ok(id) })
    }
    fn sign<'a>(&'a mut self, req: SignRequest<'a>) -> BoxFuture<'a, Result<Signed, Failure>> {
        Box::pin(async move {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.signing.0.fetch_add(1, SeqCst) + 1;
            self.signing.1.fetch_max(now, SeqCst);
            tokio::time::sleep(self.delay).await;
            self.signing.0.fetch_sub(1, SeqCst);
            if !self.cert_limit.is_empty() {
                let Some(serials) = (self.ask_certs)(self.cert_limit.clone()).await else {
                    return Err(Failure::new(CERT_LIMIT));
                };
                lock(&self.revoked).extend(serials);
            }
            let scratch = Scratch(fake_extract_dir(req.ipa));
            let app = scratch.0.join("Payload/SideStore.app");
            std::fs::create_dir_all(&app).map_err(|e| Failure::with(SIGN_FAILED, e.to_string()))?;
            std::fs::write(app.join("ALTCertificate.p12"), b"chave").map_err(|e| Failure::with(SIGN_FAILED, e.to_string()))?;
            if self.panics {
                panic!("fake isideload panic");
            }
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            lock(&self.signed).push((req.udid.to_string(), req.pairing.to_vec()));
            Ok(Signed { app, scratch })
        })
    }
}

pub use real::IsideloadSigner;

mod real {
    use super::*;
    use isideload::SideloadError;
    use isideload::anisette::remote_v3::RemoteV3AnisetteProvider;
    use isideload::auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse};
    use isideload::dev::certificates::DevelopmentCertificate;
    use isideload::dev::developer_session::DeveloperSession;
    use isideload::dev::devices::DevicesApi;
    use isideload::sideload::builder::{ExtensionsBehavior, ExtensionsBehaviorSelection, MaxCertsBehavior};
    use isideload::sideload::sideloader::Sideloader;
    use isideload::sideload::{SideloaderBuilder, TeamSelection};
    use isideload::util::callbacks::{MaxCertsCallbackBox, MaxCertsCallbackFuture};
    use isideload::util::fs_storage::FsStorage;
    use rootcause::Report;

    type NoExt = fn(&[isideload::sideload::bundle::Bundle]) -> std::future::Ready<Result<ExtensionsBehavior, Report>>;
    type Certs = Arc<Mutex<Option<Vec<DevelopmentCertificate>>>>;

    const MACHINE: &str = "Hangar";

    pub struct IsideloadSigner;

    impl AppleSigner for IsideloadSigner {
        fn login(&self, login: Login, ask: AskCode, ask_certs: AskCerts) -> BoxFuture<'_, Result<Box<dyn AppleSession>, Failure>> {
            Box::pin(async move {
                static INIT: std::sync::Once = std::sync::Once::new();
                INIT.call_once(|| if let Err(e) = isideload::init() { tracing::error!("isideload init failed: {e:?}"); });
                let Login { apple_id, password, anisette, storage } = login;
                let email = apple_id.trim().to_lowercase();
                let (email_ref, password_ref, storage_ref, ask_ref) = (&email, &password, &storage, &ask);
                let mut account = with_anisette(&anisette, |url| async move {
                    let provider = RemoteV3AnisetteProvider::new(&url, Box::new(FsStorage::new(storage_ref.clone())), "0".into())
                        .map_err(|e| Attempt::Down(e.to_string()))?;
                    // `build` já pede o `client_info`: servidor fora falha aqui, antes da senha sair.
                    let mut account = AppleAccount::builder(email_ref).anisette_provider(provider).build().await
                        .map_err(|e| Attempt::Down(e.to_string()))?;
                    match account.login(password_ref, two_factor(ask_ref.clone())).await {
                        Ok(()) => Ok(account),
                        Err(e) if e.to_string().to_lowercase().contains("anisette") => Err(Attempt::Down(e.to_string())),
                        Err(e) => Err(Attempt::Fatal(login_failure(&e))),
                    }
                })
                .await?;
                drop(password);
                let dev = DeveloperSession::from_account(&mut account).await.map_err(|e| login_failure(&e))?;
                let certs: Certs = Arc::default();
                let seen = certs.clone();
                // Limite de certificados: nada é revogado sem a pessoa escolher na tela.
                let prompt: MaxCertsCallbackBox = Box::new(move |list: Vec<DevelopmentCertificate>| -> MaxCertsCallbackFuture {
                    let shown = list.iter().filter_map(cert_info).collect();
                    *lock(&seen) = Some(list);
                    let (ask, seen) = (ask_certs.clone(), seen.clone());
                    Box::pin(async move {
                        let choice = ask(shown).await;
                        // Revogou: um erro depois disso já não é o do limite.
                        if choice.is_some() { lock(&seen).take(); }
                        Ok(choice)
                    })
                });
                let sideloader = SideloaderBuilder::new(dev, email)
                    .team_selection(TeamSelection::First)
                    .machine_name(MACHINE.into())
                    .storage(Box::new(FsStorage::new(storage)))
                    .extensions_behavior(ExtensionsBehaviorSelection::<NoExt>::Fixed(ExtensionsBehavior::ReuseMain))
                    .max_certs_behavior(MaxCertsBehavior::Prompt(prompt))
                    .build();
                Ok(Box::new(Session { sideloader, certs }) as Box<dyn AppleSession>)
            })
        }
    }

    fn two_factor(ask: AskCode) -> impl Fn(TwoFactorCallbackParams) -> BoxFuture<'static, Result<TwoFactorCallbackResponse, Report>> + Send + Sync {
        move |p: TwoFactorCallbackParams| {
            let ask = ask.clone();
            Box::pin(async move {
                // Método desconhecido: pedir de novo aos aparelhos confiáveis, onde o código chega.
                if p.unknown {
                    return Ok(TwoFactorCallbackResponse::SendToDevices);
                }
                Ok(match ask(p.last_error.is_some()).await {
                    Some(CodeReply::Submit(code)) => TwoFactorCallbackResponse::SubmitCode(code),
                    Some(CodeReply::Resend) => TwoFactorCallbackResponse::ResendCode,
                    None => TwoFactorCallbackResponse::Abort,
                })
            })
        }
    }

    struct Session {
        sideloader: Sideloader<MaxCertsCallbackBox, NoExt>,
        certs: Certs,
    }

    impl AppleSession for Session {
        fn team_id(&mut self) -> BoxFuture<'_, Result<String, Failure>> {
            Box::pin(async move { Ok(self.sideloader.get_team().await.map_err(|e| sign_failure(&e))?.team_id) })
        }

        fn sign<'a>(&'a mut self, req: SignRequest<'a>) -> BoxFuture<'a, Result<Signed, Failure>> {
            Box::pin(async move {
                let team = self.sideloader.get_team().await.map_err(|e| sign_failure(&e))?;
                // O nome vem do aparelho e pode chegar vazio; a Apple quer algum para listar o aparelho na conta.
                let device_name = Some(req.device_name.trim()).filter(|n| !n.is_empty()).unwrap_or("iPhone");
                self.sideloader.get_dev_session().ensure_device_registered(&team, device_name, req.udid, None).await
                    .map_err(|e| sign_failure(&e))?;
                // Mesmo caminho que o `Application::new` do isideload usa para extrair o IPA.
                let name = req.ipa.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let scratch = Scratch(isideload_vfs::fs::temp_dir().join(format!("{name}_extracted")));
                let signed = self.sideloader
                    .sign_app(req.ipa.to_path_buf(), Some(team), false, None::<fn(f32) -> std::future::Ready<()>>, Some(req.pairing), Some(req.udid))
                    .await;
                match signed {
                    Ok((app, _)) => Ok(Signed { app, scratch }),
                    Err(e) => Err(match lock(&self.certs).take() {
                        Some(certs) => Failure::with(CERT_LIMIT, cert_list(&certs)),
                        None => sign_failure(&e),
                    }),
                }
            })
        }
    }

    fn cert_info(c: &DevelopmentCertificate) -> Option<CertInfo> {
        Some(CertInfo {
            name: c.name.clone().unwrap_or_default(),
            machine_name: c.machine_name.clone().unwrap_or_default(),
            serial: c.serial_number.clone()?,
        })
    }

    fn cert_list(certs: &[DevelopmentCertificate]) -> String {
        certs.iter()
            .map(|c| format!("{} / {}", c.name.as_deref().unwrap_or("?"), c.machine_name.as_deref().unwrap_or("?")))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn developer_code(e: &Report) -> Option<(i64, String)> {
        e.iter_reports().find_map(|n| match n.downcast_current_context::<SideloadError>()? {
            SideloadError::DeveloperError(code, msg) | SideloadError::AuthWithMessage(code, msg) => Some((*code, msg.clone())),
            _ => None,
        })
    }

    // O rootcause não imprime o `source()` dos erros; sem ele, falha de rede vira só "error sending request".
    fn full_text(e: &Report) -> String {
        let mut t = e.to_string();
        for node in e.iter_reports() {
            let mut source = node.current_context_error_source();
            while let Some(err) = source {
                t.push_str(&format!("\n  causa: {err}"));
                source = err.source();
            }
        }
        t
    }

    pub(super) fn login_failure(e: &Report) -> Failure {
        let t = full_text(e);
        if busy(&t) {
            return Failure::with(APPLE_BUSY, t);
        }
        Failure::with(LOGIN_REFUSED, developer_code(e).map_or(t, |(code, msg)| format!("{code}: {msg}")))
    }

    pub(super) fn sign_failure(e: &Report) -> Failure {
        let t = full_text(e);
        if busy(&t) {
            return Failure::with(APPLE_BUSY, t);
        }
        if let Some((code, msg)) = developer_code(e) {
            // 9401: as 10 App IDs da semana da conta gratuita.
            if code == 9401 || msg.contains("App ID") && (msg.contains("maximum") || msg.contains("limit")) {
                return Failure::with(APPID_LIMIT, msg);
            }
            return Failure::with(SIGN_FAILED, format!("{code}: {msg}"));
        }
        if t.contains("DeviceNotFound") || t.contains("device not found") {
            return Failure::with(DISCONNECTED, t);
        }
        Failure::with(SIGN_FAILED, t)
    }

    fn busy(t: &str) -> bool {
        // O texto leva arquivo:linha do código; casar só o status HTTP, nunca o número solto.
        t.contains("503 Service Unavailable") || t.contains("Too Many Requests")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rootcause::report;

        #[test]
        fn apple_errors_map_to_screen_codes() {
            let r: Report = report!(SideloadError::DeveloperError(9401, "too many App IDs".into())).into();
            assert_eq!(sign_failure(&r).code, APPID_LIMIT);
            let r: Report = report!(SideloadError::AuthWithMessage(-20101, "Your Apple ID or password was incorrect".into())).into();
            assert_eq!(login_failure(&r), Failure::with(LOGIN_REFUSED, "-20101: Your Apple ID or password was incorrect"));
            let r: Report = report!("HTTP status server error (503 Service Unavailable)").into();
            assert_eq!(login_failure(&r).code, APPLE_BUSY);
        }

        #[derive(Debug)]
        struct Sending(std::io::Error);
        impl std::fmt::Display for Sending {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("error sending request for url")
            }
        }
        impl std::error::Error for Sending {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        #[test]
        fn network_failure_shows_the_underlying_cause() {
            let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset by peer (os error 10054)");
            let r: Report = report!(Sending(io)).context("Failed to add developer device").into();
            let detail = sign_failure(&r).detail.unwrap();
            assert!(detail.contains("error sending request for url"), "{detail}");
            assert!(detail.contains("os error 10054"), "{detail}");
            let r: Report = report!(Sending(std::io::Error::other("timed out"))).into();
            assert!(login_failure(&r).detail.unwrap().contains("timed out"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_install::job::ANISETTE_UNREACHABLE;

    #[test]
    fn configured_anisette_goes_first_without_repeating() {
        let mine = anisette_servers(&serde_json::json!({"device_install_anisette": " https://ani.sidestore.app "}));
        assert_eq!(mine, ["https://ani.sidestore.app", "https://mac-ani.jkcoxson.com", "https://ani.jkcoxson.com"]);
        assert_eq!(anisette_servers(&Value::Null), ANISETTE);
    }

    #[tokio::test]
    async fn only_anisette_failures_reach_the_next_server() {
        let servers: Vec<String> = ANISETTE.iter().map(|s| s.to_string()).collect();
        let mut seen = Vec::new();
        let r = with_anisette(&servers, |u| { seen.push(u); async { Err::<(), _>(Attempt::Fatal(Failure::new(LOGIN_REFUSED))) } }).await;
        assert_eq!((r, seen.len()), (Err(Failure::new(LOGIN_REFUSED)), 1), "senha errada não vai a outro servidor");
        let r = with_anisette(&servers, |_| async { Err::<(), _>(Attempt::Down("x".into())) }).await;
        assert_eq!(r.unwrap_err().code, ANISETTE_UNREACHABLE);
    }
}
