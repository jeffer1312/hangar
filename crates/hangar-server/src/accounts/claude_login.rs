//! Tentativas por operação; a limpeza conserva exclusividade até o auxiliar parar.
use super::{
    AccountGuard, AccountKey, GuardMode, Provider,
    catalog::{Account, AccountError, AccountService},
    claude_auth,
    claude_window::valid_operation,
    login_browser::{self, BrowserChannel, OpenRequest, Target},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs, io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Clone, Default)]
pub struct ClaudeLogins {
    attempts: Arc<AsyncMutex<HashMap<AccountKey, Arc<Attempt>>>>,
    gates: super::locks::KeyedGates,
}
struct Attempt {
    account: Account,
    key: AccountKey,
    operation: String,
    old_token: Option<Vec<u8>>,
    guard: Mutex<Option<AccountGuard>>,
    target: Option<Target>,
    browser: Mutex<Option<Value>>,
    dispatch: Mutex<Option<tokio::task::AbortHandle>>,
    channel: Option<Arc<dyn BrowserChannel>>,
}
/// Prazos da abertura e da consulta; os testes encurtam.
#[derive(Clone, Copy)]
pub struct BrowserPacing {
    pub link_wait: Duration,
    pub link_poll: Duration,
    pub result_poll: Duration,
    pub result_cap: Duration,
}
impl Default for BrowserPacing {
    fn default() -> Self {
        Self {
            link_wait: Duration::from_secs(15),
            link_poll: Duration::from_millis(250),
            result_poll: Duration::from_secs(2),
            result_cap: Duration::from_secs(600),
        }
    }
}
/// O navegador escolhido para o login.
#[derive(Clone)]
pub struct LoginTarget {
    pub target: Target,
    pub channel: Arc<dyn BrowserChannel>,
    pub pacing: BrowserPacing,
}
impl LoginTarget {
    pub fn new(target: Target, channel: Arc<dyn BrowserChannel>) -> Self {
        Self { target, channel, pacing: BrowserPacing::default() }
    }
}
pub fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| io::Error::other("operação indisponível"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
/// As janelas que o login e a renovação operam; os testes trocam o dono por um falso.
#[derive(Clone)]
pub struct WindowClient(Arc<dyn super::claude_window::WindowBackend>);
impl WindowClient {
    pub fn local(env: &super::environment::AccountEnvironment) -> Self {
        Self(Arc::new(super::claude_window::local(env)))
    }
    pub fn with(backend: Arc<dyn super::claude_window::WindowBackend>) -> Self {
        Self(backend)
    }
    pub(super) async fn call(
        &self,
        key: &AccountKey,
        operation: &str,
        action: &str,
        code: Option<&str>,
    ) -> Result<Value, AccountError> {
        // Em tarefa própria: a renovação que cede a conta larga esta chamada, e criar a janela pela
        // metade a deixaria sem as marcas que a varredura e as listas conferem.
        let (backend, key, operation, action, code) =
            (self.0.clone(), key.clone(), operation.to_owned(), action.to_owned(), code.map(str::to_owned));
        tokio::spawn(async move {
            backend.call(&key, &operation, &action, code.as_deref()).await.map_err(|reason| {
                tracing::warn!(code = reason, action, "janela de login recusou a operação");
                bridge_error()
            })
        })
        .await
        .map_err(|_| bridge_error())?
    }
    async fn open_with_browser(
        &self,
        key: &AccountKey,
        operation: &str,
        script: &std::path::Path,
    ) -> Result<Value, AccountError> {
        // Em tarefa própria pelo mesmo motivo de `call`: a janela não pode ser criada pela metade.
        let (backend, key, operation, script) = (self.0.clone(), key.clone(), operation.to_owned(), script.to_owned());
        tokio::spawn(async move {
            backend.open_with_browser(&key, &operation, &script).await.map_err(|reason| {
                tracing::warn!(code = reason, "janela de login recusou a abertura");
                bridge_error()
            })
        })
        .await
        .map_err(|_| bridge_error())?
    }
}
/// Teto do `GET` no CLI: ele só responde depois de trocar o código pelo token, com 30 s de limite.
const DELIVERY_CAP: Duration = Duration::from_secs(35);
async fn deliver_to_cli(port: u16, query: &str) -> bool {
    let Ok(http) = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DELIVERY_CAP)
        .build()
    else {
        return false;
    };
    match http.get(format!("http://127.0.0.1:{port}/callback?{query}")).send().await {
        Ok(reply) => reply.status().is_success() || reply.status().is_redirection(),
        Err(_) => false,
    }
}
fn bridge_error() -> AccountError {
    AccountError::new(
        409,
        "erro_login_ja_em_curso",
        "não foi possível controlar a janela de login",
        json!({}),
    )
}
fn cancelled(label: &str) -> AccountError {
    AccountError::new(
        409,
        "erro_login_sem_tentativa",
        format!("login da conta {label} cancelado"),
        json!({}),
    )
}
impl AccountService {
    /// Fecha as janelas de login que a instância anterior deixou: as registradas por conta e, depois,
    /// as que ficaram sem registro. Uma conta que não se recupera fica com o registro para a próxima
    /// subida e não impede as outras nem o servidor.
    pub async fn recover_claude_logins(&self, client: WindowClient) -> Result<(), AccountError> {
        // Fora do caminho da subida: multiplexador lento não pode atrasar a porta nem depender do catálogo.
        let sweeper = client.0.clone();
        tokio::spawn(async move { sweeper.sweep().await });
        for (_, account) in self.claude_accounts()? {
            if let Err(error) = self.recover_claude_login(&account, &client).await {
                tracing::warn!(
                    code = "claude_login_recovery_failed",
                    reason = error.code,
                    "login Claude da instância anterior não foi recuperado"
                );
            }
        }
        Ok(())
    }
    async fn recover_claude_login(
        &self,
        account: &Account,
        client: &WindowClient,
    ) -> Result<(), AccountError> {
        let key =
            AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
        let record = self.login_record(&key)?;
        if !record.exists() {
            return Ok(());
        }
        let _guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| AccountError::io())?;
        self.validate_claude(account, &key)?;
        let raw: Value =
            serde_json::from_slice(&fs::read(&record).map_err(|_| AccountError::io())?)
                .map_err(|_| AccountError::io())?;
        if serde_json::from_value::<AccountKey>(raw["key"].clone())
            .ok()
            .as_ref()
            != Some(&key)
        {
            return Err(AccountError::io());
        }
        let operation = raw["operation"]
            .as_str()
            .filter(|s| valid_operation(s))
            .ok_or_else(AccountError::io)?;
        client.call(&key, operation, "close", None).await?;
        self.remove_attempt_files(&key, operation);
        fs::remove_file(record).map_err(|_| AccountError::io())
    }
    /// Sobra aqui não atrapalha a tentativa seguinte, que tem outra operação: só vai ao log.
    fn remove_attempt_files(&self, key: &AccountKey, operation: &str) {
        for name in login_browser::attempt_file_names(operation) {
            let removed = self.sidecar(key, &name).map(fs::remove_file);
            if matches!(removed, Ok(Err(ref error)) if error.kind() != io::ErrorKind::NotFound) {
                tracing::warn!(code = "claude_login_file_left", "arquivo da tentativa de login não foi apagado");
            }
        }
    }
    pub(super) fn login_record(
        &self,
        key: &AccountKey,
    ) -> Result<std::path::PathBuf, AccountError> {
        self.sidecar(key, "claude-login.json")
    }
    pub async fn start_claude_login(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        self.start_claude_login_to(label, client, None).await
    }
    pub async fn start_claude_login_to(
        &self,
        label: &str,
        client: WindowClient,
        target: Option<LoginTarget>,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move { service.start_claude_owned(&label, client, target).await })
            .await
            .map_err(|_| AccountError::io())?
    }
    /// Falha aqui não encerra a tentativa: o link impresso e o código colado continuam valendo.
    async fn dispatch_browser(
        self,
        attempt: Arc<Attempt>,
        login: LoginTarget,
        url_file: std::path::PathBuf,
    ) {
        let LoginTarget { target, channel, pacing } = login;
        let set = |state: &str, reason: Option<&str>| {
            *attempt.browser.lock().unwrap() =
                Some(json!({"state":state,"reason":reason,"machine":target.machine}));
        };
        set("opening", None);
        let started = tokio::time::Instant::now();
        let mut seen = false;
        let (url, link) = loop {
            if let Ok(raw) = tokio::fs::read_to_string(&url_file).await {
                seen = true;
                let url = login_browser::clean_url(&raw).to_owned();
                if let Some(link) = login_browser::parse_authorize_url(&url) {
                    break (url, link);
                }
            }
            if started.elapsed() >= pacing.link_wait {
                set("error", Some(if seen { "erro_login_link_refused" } else { "erro_login_link_missing" }));
                return;
            }
            tokio::time::sleep(pacing.link_poll).await;
        };
        if !self.current_attempt(&attempt).await {
            return;
        }
        let remote = !channel.is_local(&target.machine);
        let request = OpenRequest {
            url,
            operation: attempt.operation.clone(),
            target: target.clone(),
            remote,
        };
        if let Err(reason) = channel.open(request).await {
            set("error", Some(&reason));
            return;
        }
        set("open", None);
        if !remote {
            return;
        }
        let started = tokio::time::Instant::now();
        let mut unreachable = false;
        let query = loop {
            if !self.current_attempt(&attempt).await {
                return;
            }
            match channel.result(&target.machine, &attempt.operation).await {
                Ok(Some(query)) => break query,
                Ok(None) => unreachable = false,
                // Queda curta passa na consulta seguinte; o log marca o começo dela, não cada consulta.
                Err(_) => {
                    if !unreachable { tracing::warn!(code = "login_result_unreachable", machine = %target.machine, "a máquina do navegador não respondeu ao pedir o retorno do login"); }
                    unreachable = true;
                }
            }
            if started.elapsed() >= pacing.result_cap {
                set("error", Some("erro_login_result_timeout"));
                return;
            }
            tokio::time::sleep(pacing.result_poll).await;
        };
        if self.current_attempt(&attempt).await && !deliver_to_cli(link.port, &query).await {
            set("error", Some("erro_login_delivery_failed"));
        }
    }
    async fn start_claude_owned(
        &self,
        label: &str,
        client: WindowClient,
        target: Option<LoginTarget>,
    ) -> Result<Value, AccountError> {
        let account = self.claude_by_label(label)?;
        let key =
            AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
        let gate = self.claude_logins.gates.gate(&key);
        let _gate = gate.lock().await;
        let previous = self.claude_logins.attempts.lock().await.get(&key).cloned();
        if let Some(previous) = previous {
            // A tela que abriu a anterior pode ter sumido sem cancelar: pedir de novo é a saída.
            tracing::warn!(
                code = "claude_login_replaced",
                "tentativa de login anterior substituída"
            );
            self.close_attempt(&previous, &client).await?;
        }
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| bridge_error())?;
        self.validate_claude(&account, &key)?;
        let old_token = claude_auth::token_signature(&account.home, true)?;
        let record = self.login_record(&key)?;
        if let Ok(bytes) = fs::read(&record) {
            let raw: Value = serde_json::from_slice(&bytes).map_err(|_| AccountError::io())?;
            if serde_json::from_value::<AccountKey>(raw["key"].clone())
                .ok()
                .as_ref()
                != Some(&key)
            {
                return Err(AccountError::io());
            }
            let operation = raw["operation"]
                .as_str()
                .filter(|s| valid_operation(s))
                .ok_or_else(AccountError::io)?;
            client.call(&key, operation, "close", None).await?;
            self.remove_attempt_files(&key, operation);
            fs::remove_file(&record).map_err(|_| AccountError::io())?;
        }
        let operation = nonce().map_err(|_| AccountError::io())?;
        let url_file = self.sidecar(&key, &login_browser::url_file_name(&operation))?;
        // O nome inteiro: o script grava ao lado dele, com o prefixo que a conta dá aos arquivos.
        let url_name = url_file.file_name().map(|name| name.to_string_lossy().into_owned()).ok_or_else(AccountError::io)?;
        // Onde o sistema não tem script, a tentativa segue sem destino, pelo link impresso.
        let script = target
            .as_ref()
            .and_then(|_| login_browser::browser_script(login_browser::current_platform(), &operation, &url_name));
        let target = target.filter(|_| script.is_some());
        crate::runtime::queue::atomic_write(
            &record,
            json!({"key":key,"operation":operation})
                .to_string()
                .as_bytes(),
        )
        .map_err(|_| AccountError::io())?;
        let attempt = Arc::new(Attempt {
            account,
            key: key.clone(),
            operation,
            old_token,
            guard: Mutex::new(Some(guard)),
            target: target.as_ref().map(|login| login.target.clone()),
            browser: Mutex::new(None),
            dispatch: Mutex::new(None),
            channel: target.as_ref().map(|login| login.channel.clone()),
        });
        self.claude_logins
            .attempts
            .lock()
            .await
            .insert(key.clone(), attempt.clone());
        let script_file = match &script {
            Some((name, content)) => {
                let file = self.sidecar(&key, name)?;
                match login_browser::write_browser_script(&file, content) {
                    Ok(()) => Some(file),
                    Err(_) => {
                        tracing::warn!(code = "claude_login_script_failed", "script do navegador não foi gravado");
                        *attempt.browser.lock().unwrap() = target.as_ref().map(|login| {
                            json!({"state":"error","reason":"erro_login_script_failed","machine":login.target.machine})
                        });
                        None
                    }
                }
            }
            None => None,
        };
        let opened = match &script_file {
            Some(file) => client.open_with_browser(&attempt.key, &attempt.operation, file).await,
            None => client.call(&attempt.key, &attempt.operation, "open", None).await,
        };
        if let Err(error) = opened {
            // A resposta perdida pode ter criado a janela: não liberar antes da limpeza.
            self.close_attempt(&attempt, &client).await?;
            return Err(error);
        }
        if let (Some(login), Some(_)) = (target, script_file) {
            let task = tokio::spawn(self.clone().dispatch_browser(attempt.clone(), login, url_file));
            *attempt.dispatch.lock().unwrap() = Some(task.abort_handle());
        }
        Ok(json!({"ok":true}))
    }
    /// Máquina e operação da tentativa com navegador escolhido; fora da trava da conta, porque a
    /// transmissão chama a outra máquina várias vezes por segundo.
    pub async fn login_view_route(&self, label: &str) -> Option<(String, String, Arc<dyn BrowserChannel>)> {
        let account = self.claude_by_label(label).ok()?;
        let key = AccountKey::new(Provider::Claude, &account.home).ok()?;
        let attempts = self.claude_logins.attempts.lock().await;
        let attempt = attempts.get(&key)?;
        Some((attempt.target.as_ref()?.machine.clone(), attempt.operation.clone(), attempt.channel.clone()?))
    }
    async fn current_attempt(&self, attempt: &Arc<Attempt>) -> bool {
        self.claude_logins
            .attempts
            .lock()
            .await
            .get(&attempt.key)
            .is_some_and(|current| Arc::ptr_eq(current, attempt))
    }
    async fn close_attempt(
        &self,
        attempt: &Arc<Attempt>,
        client: &WindowClient,
    ) -> Result<(), AccountError> {
        if !self.current_attempt(attempt).await {
            return Ok(());
        }
        // Antes de fechar: a consulta não pode entregar o retorno a um CLI que está saindo.
        if let Some(task) = attempt.dispatch.lock().unwrap().take() {
            task.abort();
        }
        client
            .call(&attempt.key, &attempt.operation, "close", None)
            .await?;
        // Sem esperar: a outra máquina pode estar fora, e aqui a trava da conta está presa.
        if let (Some(channel), Some(target)) = (attempt.channel.clone(), attempt.target.clone()) {
            let operation = attempt.operation.clone();
            tokio::spawn(async move { channel.close(&target.machine, &operation).await });
        }
        self.remove_attempt_files(&attempt.key, &attempt.operation);
        fs::remove_file(self.login_record(&attempt.key)?).map_err(|_| AccountError::io())?;
        self.claude_logins
            .attempts
            .lock()
            .await
            .remove(&attempt.key);
        attempt.guard.lock().unwrap().take();
        Ok(())
    }
    async fn observe_attempt(
        &self,
        attempt: &Arc<Attempt>,
        client: &WindowClient,
        strict: bool,
    ) -> Result<Option<Value>, AccountError> {
        let current = self.current_attempt(attempt).await;
        // Substituída: o token novo, se houver, é da tentativa que está no lugar.
        if !current
            && self
                .claude_logins
                .attempts
                .lock()
                .await
                .contains_key(&attempt.key)
        {
            return Err(cancelled(&attempt.account.id));
        }
        // Só cancelada: se a CLI já gravou o token novo, o login aconteceu e ainda precisa fechar
        // as boas-vindas e o cache; sem ele, não há o que esperar.
        let waiting = || {
            if current {
                Ok(None)
            } else {
                Err(cancelled(&attempt.account.id))
            }
        };
        self.validate_claude(&attempt.account, &attempt.key)?;
        let signature_before = claude_auth::token_signature(&attempt.account.home, strict)?;
        if !strict && (signature_before.is_none() || signature_before == attempt.old_token) {
            return waiting();
        }
        let state = self.claude_auth_guarded(&attempt.account).await;
        if !current && state["estado"] != "ok" {
            return waiting();
        }
        if state["estado"] != "ok" {
            return Err(AccountError::new(
                409,
                if strict {
                    "erro_login_sem_tentativa"
                } else {
                    "erro_login_nao_confirmado"
                },
                format!(
                    "não consegui reler o estado da conta {}: {}",
                    attempt.account.id,
                    state["motivo"].as_str().unwrap_or("indisponivel")
                ),
                json!({}),
            ));
        }
        if state["loggedIn"] != true {
            return waiting();
        }
        let signature_after = claude_auth::token_signature(&attempt.account.home, strict)?;
        if signature_after != signature_before
            || signature_after.is_none()
            || signature_after == attempt.old_token
        {
            return waiting();
        }
        // A guarda continua viva enquanto a CLI termina e o onboarding é publicado.
        client
            .call(&attempt.key, &attempt.operation, "close", None)
            .await?;
        self.complete_claude_onboarding(&attempt.account);
        self.claude_auth.invalidate(&attempt.key);
        if let Some(target) = &attempt.target
            && self.remember_login_target(&attempt.account.id, target).is_err()
        {
            tracing::warn!(code = "claude_login_target_unsaved", "destino do login não foi guardado");
        }
        self.close_attempt(attempt, client).await?;
        Ok(Some(
            json!({"ok":true,"email":state["email"],"plano":state["plano"]}),
        ))
    }
    pub async fn claude_step(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = match service.claude_by_label(&label) {
                Ok(account) => account,
                Err(_) => return Ok(json!({"etapa":"idle","url":null,"email":null,"plano":null})),
            };
            let key = AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key); let _gate = gate.lock().await;
            let Some(attempt) = service.claude_logins.attempts.lock().await.get(&key).cloned() else {
                return Ok(json!({"etapa":"idle","url":null,"email":null,"plano":null}));
            };
            let result = async {
                let pane = client.call(&key, &attempt.operation, "read", None).await?;
                if let Some(done) = service.observe_attempt(&attempt, &client, false).await? {
                    return Ok(json!({"etapa":"concluido","url":null,"email":done["email"],"plano":done["plano"]}));
                }
                let browser = attempt.browser.lock().unwrap().clone();
                let mut step = json!({"etapa":"aguardando","url":pane["url"],"email":null,"plano":null});
                // Só com destino escolhido: quem não escolheu, como o PWA e o Expo, recebe o passo de sempre.
                if let Some(browser) = browser { step["browser"] = json!(browser); }
                Ok(step)
            }.await;
            if result.is_err() { service.close_attempt(&attempt, &client).await?; }
            result
        }).await.map_err(|_| AccountError::io())?
    }
    pub async fn confirm_claude(
        &self,
        label: &str,
        code: String,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = service.claude_by_label(&label)?;
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let attempt;
            {
                let _gate = gate.lock().await;
                attempt = service
                    .claude_logins
                    .attempts
                    .lock()
                    .await
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| cancelled(&label))?;
                if let Err(error) = client
                    .call(&key, &attempt.operation, "code", Some(&code))
                    .await
                {
                    service.close_attempt(&attempt, &client).await?;
                    return Err(error);
                }
            }
            drop(code);
            let started = tokio::time::Instant::now();
            let mut poll = tokio::time::interval(Duration::from_millis(500));
            loop {
                poll.tick().await;
                let _gate = gate.lock().await;
                let result = if started.elapsed() >= Duration::from_secs(300) {
                    Err(timeout(&label))
                } else {
                    service.observe_attempt(&attempt, &client, true).await
                };
                match result {
                    Ok(Some(done)) => return Ok(done),
                    Ok(None) => (),
                    Err(error) => {
                        service.close_attempt(&attempt, &client).await?;
                        return Err(error);
                    }
                }
            }
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    pub async fn cancel_claude(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let Ok(account) = service.claude_by_label(&label) else {
                return Ok(json!({"ok":true}));
            };
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let _gate = gate.lock().await;
            let attempt = service
                .claude_logins
                .attempts
                .lock()
                .await
                .get(&key)
                .cloned();
            if let Some(attempt) = attempt {
                service.close_attempt(&attempt, &client).await?;
            }
            Ok(json!({"ok":true}))
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    pub async fn logout_claude(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = service.claude_by_label(&label)?;
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let _gate = gate.lock().await;
            let attempt = service
                .claude_logins
                .attempts
                .lock()
                .await
                .get(&key)
                .cloned();
            if let Some(attempt) = attempt {
                service.close_attempt(&attempt, &client).await?;
            }
            let _guard = service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .map_err(|_| bridge_error())?;
            service.validate_claude(&account, &key)?;
            service.claude_logout_guarded(&account).await?;
            Ok(json!({"ok":true}))
        })
        .await
        .map_err(|_| AccountError::io())?
    }
}
fn timeout(label: &str) -> AccountError {
    AccountError::new(
        504,
        "erro_login_timeout",
        format!("a conta {label} não apareceu logada em 300s"),
        json!({}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::environment::AccountEnvironment;
    use super::*;
    use futures_util::future::BoxFuture;

    /// Janela falsa: registra ação e operação e responde sem link.
    struct Recorder(tokio::sync::mpsc::UnboundedSender<(String, String)>);
    impl super::super::claude_window::WindowBackend for Recorder {
        fn call<'a>(&'a self, _key: &'a AccountKey, operation: &'a str, action: &'a str, _code: Option<&'a str>)
            -> BoxFuture<'a, Result<Value, &'static str>> {
            self.0.send((action.to_owned(), operation.to_owned())).unwrap();
            Box::pin(async { Ok(json!({"ok":true,"url":null})) })
        }
        fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(async {}) }
    }
    fn recorder() -> (WindowClient, tokio::sync::mpsc::UnboundedReceiver<(String, String)>) {
        let (sender, observed) = tokio::sync::mpsc::unbounded_channel();
        (WindowClient::with(Arc::new(Recorder(sender))), observed)
    }
    async fn action(observed: &mut tokio::sync::mpsc::UnboundedReceiver<(String, String)>) -> Option<String> {
        observed.recv().await.map(|(action, _)| action)
    }

    /// Máquina do navegador falsa: registra as aberturas e responde a consulta pela lista dada.
    #[derive(Default)]
    struct FakeChannel {
        remote: bool,
        refuse: Option<String>,
        opened: Mutex<Vec<OpenRequest>>,
        answers: Mutex<std::collections::VecDeque<Result<Option<String>, ()>>>,
    }
    impl BrowserChannel for FakeChannel {
        fn is_local(&self, _machine: &str) -> bool { !self.remote }
        fn open(&self, request: OpenRequest) -> BoxFuture<'_, Result<(), String>> {
            self.opened.lock().unwrap().push(request);
            let reply = self.refuse.clone().map_or(Ok(()), Err);
            Box::pin(async move { reply })
        }
        fn result<'a>(&'a self, _machine: &'a str, _operation: &'a str) -> BoxFuture<'a, Result<Option<String>, ()>> {
            let reply = self.answers.lock().unwrap().pop_front().unwrap_or(Ok(None));
            Box::pin(async move { reply })
        }
    }
    fn profile_target() -> Target {
        Target { machine: "casa".into(), kind: login_browser::TargetKind::ChromeProfile, profile: Some("Default".into()), label: String::new(), email: None }
    }
    fn fast(target: Target, channel: Arc<FakeChannel>) -> LoginTarget {
        LoginTarget { target, channel, pacing: BrowserPacing {
            link_wait: Duration::from_millis(200),
            link_poll: Duration::from_millis(5),
            result_poll: Duration::from_millis(5),
            result_cap: Duration::from_secs(10),
        } }
    }
    fn link(port: u16) -> String {
        format!("\"https://claude.com/cai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Fcallback&state=st4te\" \r\n")
    }
    fn logged_out_service() -> (tempfile::TempDir, AccountService, AccountKey) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
        ));
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        (root, service, key)
    }
    async fn operation_of(service: &AccountService, key: &AccountKey) -> String {
        service.claude_logins.attempts.lock().await[key].operation.clone()
    }
    fn write_link(service: &AccountService, key: &AccountKey, operation: &str, port: u16) {
        fs::write(service.sidecar(key, &login_browser::url_file_name(operation)).unwrap(), link(port)).unwrap();
    }
    async fn browser_state(service: &AccountService, client: &WindowClient, wanted: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let step = service.claude_step("work", client.clone()).await.unwrap();
            if step["browser"]["state"] == wanted { return step; }
            assert!(std::time::Instant::now() < deadline, "browser ficou em {}", step["browser"]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn local_target_opens_without_listening_or_asking_for_a_result() {
        let (_root, service, key) = logged_out_service();
        let (client, _observed) = recorder();
        let channel = Arc::new(FakeChannel::default());
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel.clone()))).await.unwrap();
        let operation = operation_of(&service, &key).await;
        let url_name = service.sidecar(&key, &login_browser::url_file_name(&operation)).unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let script = service.sidecar(&key, &login_browser::browser_script(login_browser::current_platform(), &operation, &url_name).unwrap().0).unwrap();
        assert!(script.exists());
        // O caminho de verdade: o script que o CLI chama tem que gravar onde a tentativa lê.
        #[cfg(unix)]
        assert!(std::process::Command::new(&script).arg(link(34047).trim().trim_matches('"')).status().unwrap().success());
        #[cfg(not(unix))]
        write_link(&service, &key, &operation, 34047);
        let step = browser_state(&service, &client, "open").await;
        assert_eq!(step["browser"]["machine"], "casa");
        let opened = channel.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 1);
        assert!(!opened[0].remote);
        assert_eq!(opened[0].operation, operation);
        assert!(opened[0].url.starts_with("https://claude.com/"), "aspas e fim de linha saíram");
    }

    #[tokio::test]
    async fn remote_target_survives_a_network_failure_and_delivers_the_query_to_the_cli() {
        let cli = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = cli.local_addr().unwrap().port();
        let (got, mut received) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(axum::serve(cli, axum::Router::new().route("/callback", axum::routing::get(
            move |axum::extract::RawQuery(query): axum::extract::RawQuery| {
                let got = got.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    got.send(query).unwrap();
                    axum::http::StatusCode::FOUND
                }
            },
        ))).into_future());
        let (_root, service, key) = logged_out_service();
        let (client, _observed) = recorder();
        let channel = Arc::new(FakeChannel { remote: true, ..Default::default() });
        channel.answers.lock().unwrap().extend([Err(()), Ok(None), Ok(Some("code=c0de&state=st4te".to_owned()))]);
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel.clone()))).await.unwrap();
        write_link(&service, &key, &operation_of(&service, &key).await, port);
        let query = tokio::time::timeout(Duration::from_secs(10), received.recv()).await.unwrap().unwrap();
        assert_eq!(query.as_deref(), Some("code=c0de&state=st4te"));
        assert!(channel.opened.lock().unwrap()[0].remote);
        assert_eq!(service.claude_step("work", client).await.unwrap()["browser"]["state"], "open");
    }

    #[tokio::test]
    async fn refused_machine_and_missing_link_only_report_and_keep_the_attempt() {
        let (_root, service, key) = logged_out_service();
        let (client, _observed) = recorder();
        let channel = Arc::new(FakeChannel { refuse: Some("erro_login_machine_unreachable".into()), ..Default::default() });
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel))).await.unwrap();
        write_link(&service, &key, &operation_of(&service, &key).await, 34047);
        let step = browser_state(&service, &client, "error").await;
        assert_eq!(step["browser"]["reason"], "erro_login_machine_unreachable");
        assert_eq!(step["etapa"], "aguardando");
        let channel = Arc::new(FakeChannel::default());
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel.clone()))).await.unwrap();
        let step = browser_state(&service, &client, "error").await;
        assert_eq!(step["browser"]["reason"], "erro_login_link_missing");
        assert_eq!(step["etapa"], "aguardando");
        assert!(channel.opened.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_during_the_wait_never_delivers_and_clears_the_files() {
        let cli = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = cli.local_addr().unwrap().port();
        let (_root, service, key) = logged_out_service();
        let (client, _observed) = recorder();
        let channel = Arc::new(FakeChannel { remote: true, ..Default::default() });
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel.clone()))).await.unwrap();
        let operation = operation_of(&service, &key).await;
        write_link(&service, &key, &operation, port);
        browser_state(&service, &client, "open").await;
        service.cancel_claude("work", client).await.unwrap();
        channel.answers.lock().unwrap().push_back(Ok(Some("code=c&state=st4te".to_owned())));
        assert!(tokio::time::timeout(Duration::from_millis(200), cli.accept()).await.is_err(), "o retorno não pode chegar ao CLI");
        for name in login_browser::attempt_file_names(&operation) {
            assert!(!service.sidecar(&key, &name).unwrap().exists(), "{name} ficou");
        }
    }

    #[tokio::test]
    async fn completed_login_remembers_the_target_and_a_login_without_one_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("native");
        let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        let source = "const fs=require('fs'),p=require('path');process.stdout.write(JSON.stringify({loggedIn:true,email:'fixture@example.test',subscriptionType:'pro'}));process.exit(0);";
        fs::write(script, source).unwrap();
        #[cfg(unix)]
        crate::write_test_executable(&fixture.join("claude"), format!("#!/usr/bin/env node\n{source}"));
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!("{}{separator}{}", fixture.display(), std::env::var("PATH").unwrap());
        let home = root.path().to_string_lossy().into_owned();
        let mut service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home), ("PATH".into(), path)]
                .into_iter()
                .chain(std::env::vars().filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")))
                .collect(),
        ));
        service.claude_auth.cli_timeout = Some(Duration::from_secs(3600));
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let token = |value: &str| {
            fs::write(account.home.join(".credentials.json"), json!({"claudeAiOauth":{"accessToken":value}}).to_string()).unwrap();
        };
        let (client, _observed) = recorder();
        let channel = Arc::new(FakeChannel::default());
        service.start_claude_login_to("work", client.clone(), Some(fast(profile_target(), channel))).await.unwrap();
        token("first");
        assert_eq!(service.claude_step("work", client.clone()).await.unwrap()["etapa"], "concluido");
        let stored = service.last_login_target("work").unwrap();
        assert_eq!(stored, Target { label: String::new(), email: None, ..profile_target() });
        service.start_claude_login("work", client.clone()).await.unwrap();
        token("second");
        assert_eq!(service.claude_step("work", client).await.unwrap()["etapa"], "concluido");
        assert_eq!(service.last_login_target("work"), Some(stored));
    }

    #[tokio::test]
    async fn restart_removes_the_files_of_the_previous_attempt() {
        let (_root, service, key) = logged_out_service();
        let operation = "a".repeat(32);
        fs::write(service.login_record(&key).unwrap(), json!({"key":key,"operation":operation}).to_string()).unwrap();
        for name in login_browser::attempt_file_names(&operation) {
            fs::write(service.sidecar(&key, &name).unwrap(), "x").unwrap();
        }
        let (client, _observed) = recorder();
        service.recover_claude_logins(client).await.unwrap();
        for name in login_browser::attempt_file_names(&operation) {
            assert!(!service.sidecar(&key, &name).unwrap().exists(), "{name} ficou");
        }
    }

    #[tokio::test]
    async fn new_login_replaces_abandoned_attempt() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
        ));
        let account = service
            .create(Provider::Claude, "work", |_| Ok(()))
            .unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        let (client, mut observed) = recorder();
        service
            .start_claude_login("work", client.clone())
            .await
            .unwrap();
        let (action, first) = observed.recv().await.unwrap();
        assert_eq!(action, "open");
        // A tela que abriu a primeira sumiu sem cancelar: pedir de novo é a saída dela.
        service
            .start_claude_login("work", client.clone())
            .await
            .expect("a nova tentativa substitui a abandonada");
        assert_eq!(observed.recv().await.unwrap(), ("close".to_owned(), first.clone()));
        let (action, second) = observed.recv().await.unwrap();
        assert_eq!(action, "open");
        assert_ne!(second, first);
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_err(),
            "a tentativa nova conserva a guarda da conta"
        );
    }

    #[tokio::test]
    async fn confirmation_has_300_seconds_after_protected_code_not_after_open() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("native");
        let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        let source = "const fs=require('fs'),p=require('path');fs.appendFileSync(p.join(process.env.CLAUDE_CONFIG_DIR,'status-called'), 'status\\n');process.stdout.write(JSON.stringify({loggedIn:false}));process.exit(1);";
        fs::write(script, source).unwrap();
        #[cfg(unix)]
        {
            
            crate::write_test_executable(&fixture.join("claude"), format!("#!/usr/bin/env node\n{source}"));
        }
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!(
            "{}{separator}{}",
            fixture.display(),
            std::env::var("PATH").unwrap()
        );
        let home = root.path().to_string_lossy().into_owned();
        let mut service = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.clone()),
                ("USERPROFILE".into(), home),
                ("PATH".into(), path),
            ]
            .into_iter()
            .chain(
                std::env::vars()
                    .filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")),
            )
            .collect(),
        ));
        // A CLI falsa é um `node`: no Windows do CI a subida dele passa dos 10 s de produção, e o que este teste
        // mede é a janela de 300 s.
        service.claude_auth.cli_timeout = Some(Duration::from_secs(3600));
        let account = service
            .create(Provider::Claude, "work", |_| Ok(()))
            .unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        let (client, mut observed) = recorder();
        service
            .start_claude_login("work", client.clone())
            .await
            .unwrap();
        assert_eq!(action(&mut observed).await.as_deref(), Some("open"));
        // Só avançar o relógio sem CLI em voo: o processo nativo usa I/O real.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(200)).await;
        tokio::time::resume();
        let confirming_service = service.clone();
        let confirming_client = client.clone();
        let confirming = tokio::spawn(async move {
            confirming_service
                .confirm_claude("work", "synthetic-code".into(), confirming_client)
                .await
        });
        assert_eq!(action(&mut observed).await.as_deref(), Some("code"));
        let code_sent = tokio::time::Instant::now();
        let status_called = account.home.join("status-called");
        // Teto só contra travamento: no Windows do CI o node da CLI falsa passou de 10 s para subir.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !status_called.exists() {
            assert!(
                !confirming.is_finished(),
                "a confirmação terminou antes da primeira leitura nativa"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "a CLI não recebeu a primeira leitura de confirmação"
            );
            // Dormir, não girar: o giro ocupa a CPU que a CLI esperada precisa para subir.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let gate = service.claude_logins.gates.gate(&key);
        let control = gate.lock().await;
        assert!(observed.is_empty());
        // O relógio do prazo também anda com o tempo real da primeira leitura nativa.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(299).saturating_sub(code_sent.elapsed())).await;
        tokio::time::resume();
        drop(control);
        // Teto só contra travamento: no Windows do CI o node da CLI falsa passou de 10 s para subir.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while observed.is_empty() && fs::read_to_string(&status_called).unwrap().lines().count() < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "a confirmação não avaliou a fronteira de 299 s"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let control = gate.lock().await;
        assert!(
            observed.try_recv().is_err(),
            "a confirmação não fecha a janela antes dos seus 300 s"
        );
        assert!(!confirming.is_finished());
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_err()
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::time::resume();
        drop(control);
        let error = confirming.await.unwrap().unwrap_err();
        assert_eq!(error.status, 504);
        assert_eq!(error.code, "erro_login_timeout");
        assert_eq!(action(&mut observed).await.as_deref(), Some("close"));
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_ok()
        );
        assert_eq!(
            service.claude_step("work", client).await.unwrap()["etapa"],
            "idle"
        );
    }

    #[tokio::test]
    async fn cancel_after_the_cli_wrote_the_token_still_completes_the_login() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("native");
        let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        let source = "const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;fs.appendFileSync(p.join(d,'status-called'),'status\\n');process.stdout.write(fs.readFileSync(p.join(d,'reply.json')));process.exit(1);";
        fs::write(script, source).unwrap();
        #[cfg(unix)]
        {
            
            crate::write_test_executable(&fixture.join("claude"), format!("#!/usr/bin/env node\n{source}"));
        }
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!(
            "{}{separator}{}",
            fixture.display(),
            std::env::var("PATH").unwrap()
        );
        let home = root.path().to_string_lossy().into_owned();
        let mut service = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.clone()),
                ("USERPROFILE".into(), home),
                ("PATH".into(), path),
            ]
            .into_iter()
            .chain(
                std::env::vars()
                    .filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")),
            )
            .collect(),
        ));
        // A CLI falsa é um `node`: no Windows do CI a subida dele passa dos 10 s de produção, e o que este teste
        // mede é o cancelar entre duas leituras.
        service.claude_auth.cli_timeout = Some(Duration::from_secs(3600));
        let account = service
            .create(Provider::Claude, "work", |_| Ok(()))
            .unwrap();
        fs::write(account.home.join("reply.json"), r#"{"loggedIn":false}"#).unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        let (client, mut observed) = recorder();
        service
            .start_claude_login("work", client.clone())
            .await
            .unwrap();
        assert_eq!(action(&mut observed).await.as_deref(), Some("open"));
        let confirming = tokio::spawn({
            let (service, client) = (service.clone(), client.clone());
            async move {
                service
                    .confirm_claude("work", "synthetic-code".into(), client)
                    .await
            }
        });
        assert_eq!(action(&mut observed).await.as_deref(), Some("code"));
        let status_called = account.home.join("status-called");
        // Teto só contra travamento: no Windows do CI o node da CLI falsa passou de 10 s para subir.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !status_called.exists() {
            assert!(std::time::Instant::now() < deadline, "a CLI não foi lida");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Entre duas leituras: o navegador concluiu, a CLI gravou o token, e o cancelar chega
        // antes da leitura seguinte.
        let gate = service.claude_logins.gates.gate(&key);
        let control = gate.lock().await;
        fs::write(
            account.home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"synthetic-new"}}"#,
        )
        .unwrap();
        fs::write(
            account.home.join("reply.json"),
            r#"{"loggedIn":true,"email":"fixture@example.test","subscriptionType":"pro"}"#,
        )
        .unwrap();
        drop(control);
        service.cancel_claude("work", client).await.unwrap();
        assert_eq!(action(&mut observed).await.as_deref(), Some("close"));
        let done = confirming.await.unwrap().unwrap();
        assert_eq!(done["ok"], true);
        assert_eq!(done["email"], "fixture@example.test");
        let config: Value =
            serde_json::from_slice(&fs::read(account.home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
    }

    /// Só anda depois do `Notify`, e registra ao chegar ao fim.
    struct Gated(Arc<tokio::sync::Notify>, tokio::sync::mpsc::UnboundedSender<String>);
    impl super::super::claude_window::WindowBackend for Gated {
        fn call<'a>(&'a self, _: &'a AccountKey, _: &'a str, action: &'a str, _: Option<&'a str>)
            -> BoxFuture<'a, Result<Value, &'static str>> {
            Box::pin(async move {
                self.0.notified().await;
                self.1.send(action.to_owned()).unwrap();
                Ok(json!({"ok":true}))
            })
        }
        fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(async {}) }
    }

    #[tokio::test]
    async fn dropping_a_window_call_does_not_stop_it_midway() {
        let root = tempfile::tempdir().unwrap();
        let key = AccountKey::new(Provider::Claude, root.path()).unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let (sender, mut finished) = tokio::sync::mpsc::unbounded_channel();
        let client = WindowClient::with(Arc::new(Gated(gate.clone(), sender)));
        let operation = "a".repeat(32);
        tokio::select! { biased;
            _ = client.call(&key, &operation, "open", None) => panic!("a chamada não pode terminar antes do sinal"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        gate.notify_one();
        assert_eq!(finished.recv().await.as_deref(), Some("open"));
    }

    /// A CLI responde o que está em `auth-reply.json` da conta.
    const REPLY_CLI: &str = "const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;let r;try{r=fs.readFileSync(p.join(d,'auth-reply.json'),'utf8')}catch{r=JSON.stringify({loggedIn:false})}process.stdout.write(r);process.exit(r.includes('false')?1:0);";

    fn service_with_cli(root: &std::path::Path, source: &str) -> AccountService {
        let fixture = root.join("native");
        let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(script, source).unwrap();
        #[cfg(unix)]
        crate::write_test_executable(&fixture.join("claude"), format!("#!/usr/bin/env node\n{source}"));
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!("{}{separator}{}", fixture.display(), std::env::var("PATH").unwrap());
        let home = root.to_string_lossy().into_owned();
        let mut service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home), ("PATH".into(), path)]
                .into_iter()
                .chain(std::env::vars().filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")))
                .collect(),
        ));
        service.claude_auth.cli_timeout = Some(Duration::from_secs(3600));
        service
    }

    fn write_token(home: &std::path::Path, token: &str) {
        fs::write(home.join(".credentials.json"), json!({"claudeAiOauth":{"accessToken":token}}).to_string()).unwrap();
    }

    #[tokio::test]
    async fn login_needs_a_new_token_and_an_authenticated_identity() {
        let root = tempfile::tempdir().unwrap();
        let service = service_with_cli(root.path(), REPLY_CLI);
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let (client, _observed) = recorder();
        let signed_in = r#"{"loggedIn":true,"email":"fixture@example.test","subscriptionType":"pro"}"#;
        write_token(&account.home, "synthetic-old");
        fs::write(account.home.join("auth-reply.json"), signed_in).unwrap();
        service.start_claude_login("work", client.clone()).await.unwrap();
        assert_eq!(service.claude_step("work", client.clone()).await.unwrap()["etapa"], "aguardando");
        write_token(&account.home, "synthetic-new");
        fs::write(account.home.join("auth-reply.json"), r#"{"loggedIn":false}"#).unwrap();
        assert_eq!(service.claude_step("work", client.clone()).await.unwrap()["etapa"], "aguardando");
        fs::write(account.home.join("auth-reply.json"), signed_in).unwrap();
        assert_eq!(
            service.claude_step("work", client).await.unwrap(),
            json!({"etapa":"concluido","url":null,"email":"fixture@example.test","plano":"pro"})
        );
        let config: Value = serde_json::from_slice(&fs::read(account.home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
    }

    #[tokio::test]
    async fn a_new_server_closes_only_the_abandoned_window() {
        let root = tempfile::tempdir().unwrap();
        let service = service_with_cli(root.path(), REPLY_CLI);
        service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let (client, mut observed) = recorder();
        service.start_claude_login("work", client.clone()).await.unwrap();
        let (_, abandoned) = observed.recv().await.unwrap();
        drop(service);
        let service = service_with_cli(root.path(), REPLY_CLI);
        service.recover_claude_logins(client.clone()).await.unwrap();
        assert_eq!(observed.recv().await.unwrap(), ("close".to_owned(), abandoned.clone()));
        assert_eq!(service.claude_step("work", client.clone()).await.unwrap()["etapa"], "idle");
        service.start_claude_login("work", client.clone()).await.unwrap();
        let (action, new) = observed.recv().await.unwrap();
        assert_eq!(action, "open");
        assert_ne!(new, abandoned);
        service.cancel_claude("work", client.clone()).await.unwrap();
        assert_eq!(observed.recv().await.unwrap(), ("close".to_owned(), new));
        service.cancel_claude("work", client).await.unwrap();
        assert!(observed.try_recv().is_err(), "cancelar de novo não toca em janela nenhuma");
    }

    #[tokio::test]
    async fn an_old_confirmation_cannot_clean_or_complete_the_new_attempt() {
        let root = tempfile::tempdir().unwrap();
        let service = service_with_cli(root.path(), REPLY_CLI);
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let (client, mut observed) = recorder();
        write_token(&account.home, "synthetic-old");
        fs::write(account.home.join("auth-reply.json"), r#"{"loggedIn":true}"#).unwrap();
        service.start_claude_login("work", client.clone()).await.unwrap();
        assert_eq!(action(&mut observed).await.as_deref(), Some("open"));
        let confirming = tokio::spawn({
            let (service, client) = (service.clone(), client.clone());
            async move { service.confirm_claude("work", "synthetic-code".into(), client).await }
        });
        assert_eq!(action(&mut observed).await.as_deref(), Some("code"));
        service.cancel_claude("work", client.clone()).await.unwrap();
        assert_eq!(action(&mut observed).await.as_deref(), Some("close"));
        service.start_claude_login("work", client.clone()).await.unwrap();
        let (opened, new) = observed.recv().await.unwrap();
        assert_eq!(opened, "open");
        write_token(&account.home, "synthetic-new");
        let error = confirming.await.unwrap().unwrap_err();
        assert_eq!((error.status, error.code), (409, "erro_login_sem_tentativa"));
        assert!(observed.try_recv().is_err(), "a confirmação antiga não fecha a janela nova");
        assert_eq!(service.claude_step("work", client).await.unwrap()["etapa"], "concluido");
        assert_eq!(observed.recv().await.unwrap(), ("read".to_owned(), new.clone()));
        assert_eq!(observed.recv().await.unwrap(), ("close".to_owned(), new));
    }

    #[tokio::test]
    async fn an_unreadable_identity_cleans_the_attempt_without_onboarding() {
        for reply in [r#"{"loggedIn":"true"}"#, "invalid-json"] {
            let root = tempfile::tempdir().unwrap();
            let service = service_with_cli(root.path(), REPLY_CLI);
            let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
            let (client, mut observed) = recorder();
            service.start_claude_login("work", client.clone()).await.unwrap();
            assert_eq!(action(&mut observed).await.as_deref(), Some("open"));
            write_token(&account.home, "synthetic-new");
            fs::write(account.home.join("auth-reply.json"), reply).unwrap();
            let error = service.claude_step("work", client.clone()).await.unwrap_err();
            assert_eq!(error.status, 409, "{reply}");
            assert!(!account.home.join(".claude.json").exists(), "{reply}");
            assert_eq!(action(&mut observed).await.as_deref(), Some("read"));
            assert_eq!(action(&mut observed).await.as_deref(), Some("close"));
            assert_eq!(service.claude_step("work", client).await.unwrap()["etapa"], "idle");
        }
    }

    #[tokio::test]
    async fn a_step_does_not_conclude_when_the_credential_changes_during_the_identity_read() {
        let root = tempfile::tempdir().unwrap();
        let source = "const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;const m=p.join(d,'probe-raced');if(!fs.existsSync(m)){fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-after-probe'}}));fs.writeFileSync(m,'1');process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));}else{process.stdout.write(JSON.stringify({loggedIn:false}));}";
        let service = service_with_cli(root.path(), source);
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let (client, mut observed) = recorder();
        write_token(&account.home, "synthetic-initial");
        service.start_claude_login("work", client.clone()).await.unwrap();
        write_token(&account.home, "synthetic-before-probe");
        assert_eq!(service.claude_step("work", client).await.unwrap()["etapa"], "aguardando");
        assert!(account.home.join("probe-raced").exists());
        assert!(!account.home.join(".claude.json").exists());
        assert_eq!(action(&mut observed).await.as_deref(), Some("open"));
        assert_eq!(action(&mut observed).await.as_deref(), Some("read"));
        assert!(observed.try_recv().is_err(), "a janela segue aberta");
    }

    #[tokio::test]
    async fn a_confirmation_does_not_accept_an_identity_read_before_the_token_changed() {
        let root = tempfile::tempdir().unwrap();
        let source = "const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;const m=p.join(d,'probe-count');const c=fs.existsSync(m)?Number(fs.readFileSync(m,'utf8'))+1:1;if(c===1){fs.writeFileSync(p.join(d,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'synthetic-new'}}));process.stdout.write(JSON.stringify({loggedIn:true,email:'old@example.test'}));}else{process.stdout.write(JSON.stringify({loggedIn:false}));}fs.writeFileSync(m,String(c));";
        let service = service_with_cli(root.path(), source);
        let account = service.create(Provider::Claude, "work", |_| Ok(())).unwrap();
        let (client, _observed) = recorder();
        write_token(&account.home, "synthetic-old");
        service.start_claude_login("work", client.clone()).await.unwrap();
        let confirming = tokio::spawn({
            let (service, client) = (service.clone(), client.clone());
            async move { service.confirm_claude("work", "synthetic-code".into(), client).await }
        });
        let marker = account.home.join("probe-count");
        // Teto só contra travamento: no Windows do CI o node da CLI falsa passou de 10 s para subir.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !fs::read_to_string(&marker).is_ok_and(|count| !count.is_empty() && count != "1") {
            assert!(!confirming.is_finished(), "a confirmação terminou antes da releitura");
            assert!(std::time::Instant::now() < deadline, "a releitura nativa não avançou");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        service.cancel_claude("work", client).await.unwrap();
        assert_eq!(confirming.await.unwrap().unwrap_err().status, 409);
        assert!(!account.home.join(".claude.json").exists());
    }

    #[tokio::test]
    async fn one_unrecoverable_login_does_not_block_the_others() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
        ));
        let broken = service.create(Provider::Claude, "broken", |_| Ok(())).unwrap();
        let healthy = service.create(Provider::Claude, "healthy", |_| Ok(())).unwrap();
        let broken_record = service
            .login_record(&AccountKey::new(Provider::Claude, &broken.home).unwrap())
            .unwrap();
        fs::write(&broken_record, b"{ilegivel").unwrap();
        let healthy_key = AccountKey::new(Provider::Claude, &healthy.home).unwrap();
        let healthy_record = service.login_record(&healthy_key).unwrap();
        fs::write(
            &healthy_record,
            json!({"key":healthy_key,"operation":"a".repeat(32)}).to_string(),
        )
        .unwrap();
        let (client, mut observed) = recorder();
        assert!(service.recover_claude_logins(client).await.is_ok());
        assert_eq!(action(&mut observed).await.as_deref(), Some("close"));
        assert!(!healthy_record.exists(), "a conta saudável não foi recuperada");
        assert!(broken_record.exists(), "o registro ilegível fica para a próxima subida");
    }
}
