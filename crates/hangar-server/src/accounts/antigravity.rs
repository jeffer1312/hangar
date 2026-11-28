//! Contas Google do Antigravity (agy): a padrão, no ~/.gemini, e as adicionais, cada uma numa pasta
//! ~/.antigravity-<nome> usada como GEMINI_HOME. O Hangar nunca lê o token: o login é o do próprio agy
//! (link + código colado), "logado" é a resposta do agy models, e sair é o logout do ACP.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use hangar_antigravity::{env::ProcessEnv, login, paths, probe, terms::{AnswerError, Driver, Stage}, Account};
use hangar_core::terminal_state::AgyTermsKey;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};

use super::catalog::{AccountError, valid_name};
use super::http::{Json, error};
use super::private_window::{PrivateWindows, ProcessWindowMux};

const PREFIX: &str = "/api/antigravity-contas";
pub const DEFAULT_ID: &str = "default";
const PROBE_LIMIT: Duration = Duration::from_secs(20);
const SIGNED_TTL: Duration = Duration::from_secs(60);

/// Uma tentativa de login por conta: (attempt_id, início, processo do agy --login).
static ATTEMPTS: LazyLock<Mutex<HashMap<String, (String, Instant, login::Attempt)>>> = LazyLock::new(Default::default);
/// Tela fechada sem cancelar deixaria o `agy --login` esperando o código para sempre.
const LOGIN_LIMIT: Duration = Duration::from_secs(10 * 60);

fn attempts() -> std::sync::MutexGuard<'static, HashMap<String, (String, Instant, login::Attempt)>> {
    let mut attempts = ATTEMPTS.lock().unwrap_or_else(|e| e.into_inner());
    attempts.retain(|_, (_, started, attempt)| {
        let alive = started.elapsed() < LOGIN_LIMIT;
        if !alive { attempt.cancel(); }
        alive
    });
    attempts
}
/// A janela escondida dos Termos: o `agy` da conta padrão conduzido pelo servidor, uma tentativa por vez.
const TERMS_PREFIX: &str = "term-agy-terms-";
/// Larga o bastante para a caixa dos Termos, a tela de cores e "You are currently not signed in." não quebrarem.
const TERMS_WIDTH: u16 = 240;
const TERMS_TICK: Duration = Duration::from_millis(500);
const TERMINAL_FAILED: &str = "termos_terminal_falhou";
/// O prazo total da tentativa; o `termos_prazo` do condutor é o de 60 s sem tela.
const EXPIRED: &str = "termos_expirou";
static TERMS: LazyLock<TermsAttempts> = LazyLock::new(Default::default);

#[derive(Default)]
pub(crate) struct TermsAttempts(tokio::sync::Mutex<Option<TermsAttempt>>);

struct TermsAttempt {
    id: String,
    win: Arc<PrivateWindows>,
    live: Arc<Live>,
    stop: Arc<Notify>,
    /// `Err`: a janela não fechou.
    task: tokio::task::JoinHandle<Result<(), &'static str>>,
}

/// O condutor e a etapa que o GET mostra; a resposta e a tarefa mudam os dois sob a mesma trava.
struct Live { driver: Mutex<Driver>, stage: watch::Sender<Stage> }

impl Live {
    /// Quem espera a etapa só é avisado quando ela muda.
    fn publish(&self, stage: &Stage) {
        self.stage.send_if_modified(|current| {
            let changed = current != stage;
            if changed { *current = stage.clone(); }
            changed
        });
    }
}

impl TermsAttempt {
    /// Para a tarefa e espera ela fechar a janela; `Err` = a janela ficou aberta.
    async fn finish(self) -> Result<(), &'static str> {
        self.stop.notify_one();
        match self.task.await {
            Ok(closed) => closed,
            Err(error) => {
                tracing::warn!(code = "terms_task_crashed", %error, "Termos do agy: a tarefa da tentativa caiu");
                // A tarefa que caiu não fechou a sua janela.
                self.win.kill(&self.win.name(&self.id)).await
            }
        }
    }
}

#[cfg(test)]
impl TermsAttempts {
    fn watch(&self, id: &str) -> Option<watch::Receiver<Stage>> {
        self.0.try_lock().ok()?.as_ref().filter(|attempt| attempt.id == id).map(|attempt| attempt.live.stage.subscribe())
    }
}

/// O agy models leva ~1 s: a lista guarda a resposta por conta.
static SIGNED: LazyLock<Mutex<HashMap<PathBuf, (Instant, Option<bool>)>>> = LazyLock::new(Default::default);

fn fail(status: u16, code: &'static str, message: &str) -> Response {
    error(AccountError::new(status, code, message, json!({})))
}

/// id da rota → conta. "default" é a do ~/.gemini; o resto é o <nome> de ~/.antigravity-<nome>.
fn account_of(home: &Path, id: &str) -> Option<(Account, Option<PathBuf>)> {
    if id == DEFAULT_ID { return Some((Account::default(), None)); }
    if !valid_name(id) { return None; }
    let dir = home.join(format!("{}{id}", paths::ACCOUNT_PREFIX));
    paths::is_account_dir(&dir).then(|| (Account::extra(dir.clone()), Some(dir)))
}

/// A conta do id, para quem está fora deste módulo (catálogo de modelos).
pub(crate) fn account(home: &Path, id: &str) -> Option<Account> { account_of(home, id).map(|(account, _)| account) }

fn id_of(account: &Account) -> String {
    match &account.gemini_home {
        None => DEFAULT_ID.into(),
        Some(dir) => dir.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(paths::ACCOUNT_PREFIX)).unwrap_or_default().into(),
    }
}

pub fn matches(method: &Method, path: &str) -> bool {
    if path == PREFIX { return matches!(*method, Method::GET | Method::POST); }
    let Some(tail) = path.strip_prefix(PREFIX).and_then(|t| t.strip_prefix('/')) else { return false };
    let parts: Vec<&str> = tail.split('/').collect();
    match parts.as_slice() {
        [id] => !id.is_empty() && *method == Method::DELETE,
        [id, "login"] => !id.is_empty() && matches!(*method, Method::GET | Method::POST | Method::DELETE),
        [id, "login", "codigo"] | [id, "logout"] | [id, "termos", "resposta"] => !id.is_empty() && *method == Method::POST,
        [id, "termos"] => !id.is_empty() && matches!(*method, Method::GET | Method::POST | Method::DELETE),
        _ => false,
    }
}

pub async fn public(state: Arc<crate::routes::AppState>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query: HashMap<String, String> = form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let home = state.accounts.env.home.clone();
    let body: Value = match axum::body::to_bytes(request.into_body(), 16384).await {
        Ok(bytes) if bytes.is_empty() => Value::Null,
        Ok(bytes) => match serde_json::from_slice(&bytes) { Ok(v) => v, Err(_) => return fail(422, "erro_corpo_invalido", "corpo inválido") },
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    if path == PREFIX {
        return if method == Method::GET { list(&state, &home).await } else { create(&home, &body) };
    }
    let tail = path.trim_start_matches(PREFIX).trim_start_matches('/');
    let parts: Vec<&str> = tail.split('/').collect();
    let Ok(id) = percent_encoding::percent_decode_str(parts[0]).decode_utf8() else { return StatusCode::BAD_REQUEST.into_response() };
    let id = id.into_owned();
    let Some((account, dir)) = account_of(&home, &id) else { return fail(404, "erro_conta_antigravity_inexistente", "conta do Antigravity não encontrada") };
    let program = || crate::cli_probe::resolve_program("agy", &state.accounts.env);
    let env = hangar_antigravity::env::process_env(&account, state.accounts.env.base.keys().cloned());
    match (&parts[1..], method) {
        ([], Method::DELETE) => delete(&id, dir),
        (["login"] | ["login", "codigo"], Method::POST) | (["termos"] | ["termos", "resposta"], _) if dir.is_some() => only_default(),
        (["termos"], Method::POST) => {
            if program().is_none() { return fail(409, "erro_agy_ausente", "o agy não está instalado nesta máquina") }
            start_terms(&TERMS, Arc::new(terms_windows(&state.accounts.env)), &home, env).await
        }
        (["termos"], Method::GET) => terms_status(&TERMS, query.get("attempt_id").map(String::as_str)).await,
        (["termos"], Method::DELETE) => cancel_terms(&TERMS, &terms_windows(&state.accounts.env), query.get("attempt_id").map(String::as_str)).await,
        (["termos", "resposta"], Method::POST) => answer_terms(&TERMS, &body).await,
        (["logout"], Method::POST) => {
            let Some(program) = program() else { return fail(409, "erro_agy_ausente", "o agy não está instalado nesta máquina") };
            logout(&program, &home, env, &account).await
        }
        (["login"], Method::POST) => {
            let Some(program) = program() else { return fail(409, "erro_agy_ausente", "o agy não está instalado nesta máquina") };
            start_login(&id, &program, &home, env)
        }
        (["login"], Method::GET) => login_status(&id, query.get("attempt_id").map(String::as_str), &home),
        (["login"], Method::DELETE) => cancel_login(&id, query.get("attempt_id").map(String::as_str)),
        (["login", "codigo"], Method::POST) => submit_code(&id, &body),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn list(state: &crate::routes::AppState, home: &Path) -> Response {
    let program = crate::cli_probe::resolve_program("agy", &state.accounts.env);
    // Como em `/api/credenciais`: máquina sem o agy usado não ganha a linha da conta padrão.
    let accounts: Vec<Account> = paths::known_accounts(home).into_iter()
        .filter(|account| !account.is_default() || account.data_dir(home).is_dir()).collect();
    let base_keys: Vec<String> = state.accounts.env.base.keys().cloned().collect();
    let checks = accounts.iter().map(|account| {
        let (account, home, program, keys) = (account.clone(), home.to_owned(), program.clone(), base_keys.clone());
        tokio::task::spawn_blocking(move || {
            let signed = program.as_deref().and_then(|program| signed_in(program, &home, &account, keys));
            let in_use = in_use(&account);
            (account, signed, in_use)
        })
    });
    let mut rows = Vec::new();
    let results = futures_util::future::join_all(checks).await;
    for (account, check) in accounts.iter().zip(results) {
        // Conferência que caiu não tira a conta da lista: ela segue com o login desconhecido.
        let (signed, in_use) = match check {
            Ok((_, signed, in_use)) => (signed, in_use),
            Err(error) => {
                tracing::warn!(code = "agy_account_check_failed", %error, "conferência da conta do Antigravity caiu");
                (None, false)
            }
        };
        rows.push(json!({
            "id": id_of(&account),
            "padrao": account.is_default(),
            "pasta": account.gemini_dir(home).to_string_lossy(),
            "credencial": format!("antigravity:{}", super::catalog::resolved(&account.gemini_dir(home)).to_string_lossy()),
            "logado": signed,
            "termos_aceitos": probe::onboarding_complete(&account, home),
            "em_uso": in_use,
        }));
    }
    Json(json!({"instalado": program.is_some(), "contas": rows})).into_response()
}

/// "logado" com cache curto. Falha ou prazo estourado vira desconhecido (null), nunca "não logado".
fn signed_in(program: &Path, home: &Path, account: &Account, keys: Vec<String>) -> Option<bool> {
    let dir = account.gemini_dir(home);
    if let Some((at, value)) = SIGNED.lock().unwrap_or_else(|e| e.into_inner()).get(&dir).copied() && at.elapsed() < SIGNED_TTL {
        return value;
    }
    let env = hangar_antigravity::env::process_env(account, keys);
    let value = match probe::signed_in(program, home, &env, PROBE_LIMIT) {
        Ok(value) => value,
        // Erro de execução não entra no cache: a próxima leitura tenta de novo.
        Err(error) => {
            tracing::warn!(code = "agy_signed_probe_failed", %error, "agy models não rodou para conferir o login");
            return None;
        }
    };
    SIGNED.lock().unwrap_or_else(|e| e.into_inner()).insert(dir, (Instant::now(), value));
    value
}

/// Só o que a tela de contas já guardou: a lista de credenciais não roda o agy.
pub(crate) fn cached_signed(account: &Account, home: &Path) -> Option<bool> {
    SIGNED.lock().unwrap_or_else(|e| e.into_inner()).get(&account.gemini_dir(home)).and_then(|(at, value)| (at.elapsed() < SIGNED_TTL).then_some(*value)).flatten()
}

/// Nome da conta nas listas: "Google (padrão)" ou o nome da pasta adicional.
pub(crate) fn label(account: &Account) -> String {
    if account.is_default() { "Google (padrão)".into() } else { id_of(account) }
}

fn forget_signed(account: &Account, home: &Path) {
    SIGNED.lock().unwrap_or_else(|e| e.into_inner()).remove(&account.gemini_dir(home));
}

/// O `agy` com terminal ignora o `GEMINI_HOME`: o login de uma conta adicional gravava na padrão. A pasta e o
/// login por conta ficam no `hangar-antigravity` para o modo sem terminal, ainda não medido.
fn only_default() -> Response {
    fail(409, ADDITIONAL, "o Antigravity só usa a conta padrão (a do ~/.gemini) por enquanto")
}
const ADDITIONAL: &str = "erro_conta_antigravity_adicional";

fn create(_home: &Path, _body: &Value) -> Response { only_default() }

#[cfg(test)]
fn create_folder(home: &Path, body: &Value) -> Response {
    let Some(name) = body["name"].as_str().filter(|n| valid_name(n) && *n != DEFAULT_ID) else {
        return fail(422, "erro_nome_conta_invalido", "use letras minúsculas, números, - e _ (até 32)");
    };
    let dir = home.join(format!("{}{name}", paths::ACCOUNT_PREFIX));
    if dir.exists() { return fail(409, "erro_conta_existe", "já existe uma pasta com esse nome"); }
    let marker = json!({"version": 1, "id": name}).to_string();
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(paths::MARKER), marker)) {
        return fail(500, "erro_conta_nao_criada", &e.to_string());
    }
    Json(json!({"id": name, "pasta": dir.to_string_lossy()})).into_response()
}

fn delete(id: &str, dir: Option<PathBuf>) -> Response {
    let Some(dir) = dir else { return fail(409, "erro_conta_padrao", "a conta padrão do agy não se apaga pelo Hangar") };
    if in_use(&Account::extra(dir.clone())) { return fail(409, "erro_conta_em_uso", "há uma sessão do agy aberta nesta conta"); }
    if let Some((_, _, mut attempt)) = attempts().remove(id) { attempt.cancel(); }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => fail(500, "erro_conta_nao_apagada", &e.to_string()),
    }
}

fn start_login(id: &str, program: &Path, home: &Path, env: hangar_antigravity::env::ProcessEnv) -> Response {
    let attempt = match login::Attempt::start(program, home, env) {
        Ok(attempt) => attempt,
        Err(e) => return fail(500, "erro_login_nao_iniciou", &e.to_string()),
    };
    let attempt_id = crate::mods::state::random_hex(16);
    // Login novo encerra o anterior da mesma conta.
    if let Some((_, _, mut old)) = attempts().insert(id.to_owned(), (attempt_id.clone(), Instant::now(), attempt)) { old.cancel(); }
    Json(json!({"attempt_id": attempt_id, "estado": "iniciando"})).into_response()
}

fn login_status(id: &str, attempt_id: Option<&str>, home: &Path) -> Response {
    let mut attempts = attempts();
    let Some((current, _, attempt)) = attempts.get_mut(id).filter(|(current, _, _)| Some(current.as_str()) == attempt_id) else {
        return fail(404, "erro_tentativa_inexistente", "tentativa de login não encontrada");
    };
    let current = current.clone();
    let reply = match attempt.status() {
        login::Status::Starting => json!({"attempt_id": current, "estado": "iniciando"}),
        login::Status::Waiting { link } => json!({"attempt_id": current, "estado": "aguardando_codigo", "link": link}),
        login::Status::Done { ok, tail } => {
            attempts.remove(id);
            if let Some((account, _)) = account_of(home, id) {
                forget_signed(&account, home);
                // A conta nova ganha o hook de estado já no primeiro login.
                if ok {
                    let installed = crate::harness_setup::agy_hooks::install(home, &account.hooks_path(home));
                    if let Err(error) = installed {
                        tracing::warn!(code = "agy_hooks_after_login_failed", %error, "hook do Antigravity não gravado depois do login");
                    }
                }
            }
            json!({"attempt_id": current, "estado": if ok { "concluido" } else { "falhou" }, "saida": tail})
        }
    };
    Json(reply).into_response()
}

fn submit_code(id: &str, body: &Value) -> Response {
    let mut attempts = attempts();
    let Some((_, _, attempt)) = attempts.get_mut(id).filter(|(current, _, _)| body["attempt_id"].as_str() == Some(current.as_str())) else {
        return fail(404, "erro_tentativa_inexistente", "tentativa de login não encontrada");
    };
    match attempt.submit(body["codigo"].as_str().unwrap_or_default()) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(login::SubmitError::Code(login::CodeError::Empty)) => fail(422, "erro_codigo_vazio", "cole o código mostrado pelo Google"),
        Err(login::SubmitError::Code(login::CodeError::Invalid)) => fail(422, "erro_codigo_invalido", "isso não parece o código nem o endereço da página do Antigravity"),
        Err(login::SubmitError::Closed) => fail(409, "erro_codigo_ja_enviado", "o código desta tentativa já foi enviado"),
        Err(login::SubmitError::Io(e)) => fail(500, "erro_codigo_nao_enviado", &e.to_string()),
    }
}

fn cancel_login(id: &str, attempt_id: Option<&str>) -> Response {
    let mut attempts = attempts();
    if attempts.get(id).is_some_and(|(current, _, _)| Some(current.as_str()) == attempt_id)
        && let Some((_, _, mut attempt)) = attempts.remove(id) {
        attempt.cancel();
    }
    Json(json!({"ok": true})).into_response()
}

fn terms_windows(env: &super::environment::AccountEnvironment) -> PrivateWindows {
    let diary = crate::diag::diary_dir_in(env);
    // Dono = a pasta do agy desta HOME: outro servidor no mesmo multiplexador tem outra.
    let owner = Account::default().gemini_dir(&env.home).to_string_lossy().into_owned();
    let terminal = super::claude_window::terminal_env(env);
    PrivateWindows::new(Box::new(ProcessWindowMux::new(&terminal, diary.clone())), &terminal, TERMS_PREFIX, owner, diary)
}

/// Na subida: a janela dos Termos que a instância anterior deixou.
pub async fn sweep_terms(env: super::environment::AccountEnvironment) {
    let slot = TERMS.0.lock().await;
    terms_windows(&env).sweep(|id| slot.as_ref().is_none_or(|attempt| attempt.id != id)).await;
}

/// Nova tentativa encerra a anterior (e fecha a janela dela) antes de abrir a sua.
async fn start_terms(attempts: &TermsAttempts, win: Arc<PrivateWindows>, home: &Path, env: ProcessEnv) -> Response {
    let mut slot = attempts.0.lock().await;
    let closed = match slot.take() { Some(old) => old.finish().await.is_ok(), None => false };
    // O que sobrou com o prefixo é de tentativa cujo fechamento falhou (já registrado), ou de outra instância.
    if !closed { win.sweep(|_| true).await; }
    let id = crate::mods::state::random_hex(16);
    let started = tokio::time::Instant::now();
    let live = Arc::new(Live { driver: Mutex::new(Driver::new(started.into_std())), stage: watch::channel(Stage::Starting).0 });
    let stop = Arc::new(Notify::new());
    let task = tokio::spawn(conduct(win.clone(), id.clone(), home.to_owned(), env, live.clone(), stop.clone(), started));
    let reply = stage_json(&id, &Stage::Starting);
    *slot = Some(TermsAttempt { id, win, live, stop, task });
    Json(reply).into_response()
}

/// A tarefa da tentativa: toda saída tenta fechar a janela, e a etapa final sai depois disso. A janela que não
/// fechou fica registrada e é devolvida (o DELETE responde erro); a varredura seguinte a derruba.
async fn conduct(win: Arc<PrivateWindows>, id: String, home: PathBuf, env: ProcessEnv, live: Arc<Live>, stop: Arc<Notify>, started: tokio::time::Instant)
    -> Result<(), &'static str> {
    let name = win.name(&id);
    let ended = drive(&win, &name, &home, &env, &live, &stop, started).await;
    let closed = win.kill(&name).await;
    if let Err(code) = closed { win.note(code); }
    if ended {
        let driver = live.driver.lock().unwrap_or_else(|e| e.into_inner());
        live.publish(driver.stage());
    }
    closed
}

/// A causa vai ao log e ao diário; a pessoa vê `termos_terminal_falhou`.
fn window_failed(win: &PrivateWindows, code: &'static str) -> &'static str {
    tracing::warn!(code, "Termos do agy: a janela escondida falhou");
    win.record("conta.termos_janela", code);
    TERMINAL_FAILED
}

/// Até o fim (`true`, com a etapa final gravada no condutor) ou o pedido de parar (`false`).
async fn drive(win: &PrivateWindows, name: &str, home: &Path, env: &ProcessEnv, live: &Live, stop: &Notify, started: tokio::time::Instant) -> bool {
    // Gravado no condutor, sob a trava da resposta: nada mais é respondido nem apertado.
    let end = |code: &'static str| {
        live.driver.lock().unwrap_or_else(|e| e.into_inner()).fail(code);
        true
    };
    let command = env.command(win.windows(), vec!["agy".into()]);
    if let Err(code) = win.spawn(name, &home.to_string_lossy(), TERMS_WIDTH, &env.set).await { return end(window_failed(win, code)); }
    if let Err(code) = win.type_line(name, &command.join(" ")).await { return end(window_failed(win, code)); }
    let account = Account::default();
    let mut unread = false;
    loop {
        tokio::select! {
            () = stop.notified() => return false,
            () = tokio::time::sleep(TERMS_TICK) => {}
        }
        let now = tokio::time::Instant::now();
        if now.duration_since(started) >= LOGIN_LIMIT {
            tracing::warn!(code = EXPIRED, "Termos do agy: a tentativa passou do prazo");
            return end(EXPIRED);
        }
        // Tela que não se lê com a janela de pé vale como tela vazia: o prazo do condutor continua correndo.
        let screen = match win.capture(name).await {
            Ok(Some(screen)) => { unread = false; screen }
            Ok(None) => {
                if !unread { tracing::warn!(code = "window_capture_failed", "Termos do agy: a tela não foi lida"); }
                unread = true;
                String::new()
            }
            Err(code) => return end(window_failed(win, code)),
        };
        let accepted = probe::onboarding_complete(&account, home);
        let key = {
            let mut driver = live.driver.lock().unwrap_or_else(|e| e.into_inner());
            let key = driver.observe(&screen, accepted, now.into_std());
            // O login também encerra: os clientes passam ao login de sempre e não leem mais esta tentativa.
            if matches!(driver.stage(), Stage::Done | Stage::Failed(_) | Stage::Login) { return true; }
            live.publish(driver.stage());
            key
        };
        let sent = match key {
            None => true,
            Some(AgyTermsKey::Enter) => win.enter(name).await,
            Some(key) => win.key(name, key.name()).await,
        };
        if !sent { return end(window_failed(win, "window_key_failed")); }
    }
}

fn missing_attempt() -> Response { fail(404, "erro_tentativa_inexistente", "tentativa dos Termos não encontrada") }

/// A etapa como o POST e o GET dos Termos a mostram.
fn stage_json(id: &str, stage: &Stage) -> Value {
    match stage {
        Stage::Starting => json!({"attempt_id": id, "estado": "iniciando"}),
        Stage::Terms { text } => json!({"attempt_id": id, "estado": "termos", "texto": text}),
        Stage::Answering => json!({"attempt_id": id, "estado": "respondendo"}),
        Stage::Login => json!({"attempt_id": id, "estado": "login"}),
        Stage::Done => json!({"attempt_id": id, "estado": "concluido"}),
        Stage::Failed(code) => json!({"attempt_id": id, "estado": "falhou", "codigo": code}),
    }
}

async fn terms_status(attempts: &TermsAttempts, attempt_id: Option<&str>) -> Response {
    let slot = attempts.0.lock().await;
    let Some(attempt) = slot.as_ref().filter(|attempt| Some(attempt.id.as_str()) == attempt_id) else { return missing_attempt() };
    let reply = stage_json(&attempt.id, &attempt.live.stage.borrow());
    Json(reply).into_response()
}

/// A opção que a pessoa tocou; a tecla sai na próxima volta da tarefa.
async fn answer_terms(attempts: &TermsAttempts, body: &Value) -> Response {
    let slot = attempts.0.lock().await;
    let Some(attempt) = slot.as_ref().filter(|attempt| body["attempt_id"].as_str() == Some(attempt.id.as_str())) else { return missing_attempt() };
    let invalid = || fail(422, "erro_corpo_invalido", "corpo inválido");
    let Some(option) = body["opcao"].as_u64().and_then(|n| usize::try_from(n).ok()) else { return invalid() };
    let outside = || fail(409, "erro_termos_fora_da_tela", "a tela dos Termos não está aberta");
    let mut driver = attempt.live.driver.lock().unwrap_or_else(|e| e.into_inner());
    // A etapa publicada também: a tentativa que acabou nunca volta a "respondendo".
    if !matches!(*attempt.live.stage.borrow(), Stage::Terms { .. }) { return outside(); }
    match driver.answer(option) {
        Ok(()) => {
            attempt.live.publish(driver.stage());
            Json(json!({"ok": true})).into_response()
        }
        Err(AnswerError::OutsideTerms) => outside(),
        Err(AnswerError::InvalidOption) => invalid(),
    }
}

/// A tentativa que fechou a própria janela responde ok. Senão a varredura fecha o que sobrou de uma que falhou: ela
/// já saiu do mapa, e o novo Cancelar tenta de novo em vez de responder ok com a janela aberta. A janela da
/// tentativa viva fica.
async fn cancel_terms(attempts: &TermsAttempts, win: &PrivateWindows, attempt_id: Option<&str>) -> Response {
    let mut slot = attempts.0.lock().await;
    let mut own = None;
    if slot.as_ref().is_some_and(|attempt| Some(attempt.id.as_str()) == attempt_id) && let Some(attempt) = slot.take() {
        let attempt_win = attempt.win.clone();
        if attempt.finish().await.is_ok() { return Json(json!({"ok": true})).into_response(); }
        own = Some(attempt_win);
    }
    let win = own.as_deref().unwrap_or(win);
    if !win.sweep(|id| slot.as_ref().is_none_or(|attempt| attempt.id != id)).await {
        return fail(500, "erro_termos_janela_aberta", "a janela escondida dos Termos não fechou");
    }
    Json(json!({"ok": true})).into_response()
}

async fn logout(program: &Path, home: &Path, env: hangar_antigravity::env::ProcessEnv, account: &Account) -> Response {
    let (program, cwd) = (program.to_owned(), home.to_owned());
    let done = tokio::task::spawn_blocking(move || probe::logout(&program, &cwd, &env, PROBE_LIMIT)).await;
    forget_signed(account, home);
    // A cota guardada é da conta que saiu; a próxima statusline grava a da conta que entrar.
    match std::fs::remove_file(account.quota_path(home)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound =>
            tracing::warn!(code = "agy_quota_forget_failed", %error, "cota do Antigravity da conta que saiu não apagada"),
        _ => {}
    }
    match done {
        Ok(Ok(true)) => Json(json!({"ok": true})).into_response(),
        Ok(Ok(false)) => fail(502, "erro_logout_recusado", "o agy não confirmou a saída da conta"),
        Ok(Err(e)) => fail(500, "erro_logout_nao_rodou", &e.to_string()),
        Err(_) => fail(500, "erro_logout_nao_rodou", "a saída da conta não terminou"),
    }
}

/// Há um processo agy aberto nesta conta? (pelo GEMINI_HOME do processo; a conta padrão é a sem ele)
#[cfg(target_os = "linux")]
fn in_use(account: &Account) -> bool {
    let wanted = account.gemini_home.as_ref().map(|d| d.to_string_lossy().into_owned());
    std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|entry| {
        let dir = entry.path();
        let agy = std::fs::read_to_string(dir.join("comm")).is_ok_and(|comm| comm.trim() == "agy");
        agy && std::fs::read(dir.join("environ")).is_ok_and(|raw| {
            let home = raw.split(|b| *b == 0).find_map(|var| var.strip_prefix(b"GEMINI_HOME=")).map(|v| String::from_utf8_lossy(v).into_owned());
            home.filter(|h| !h.is_empty()) == wanted
        })
    })
}

#[cfg(not(target_os = "linux"))]
fn in_use(_account: &Account) -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routes_are_only_the_account_family() {
        assert!(matches(&Method::GET, PREFIX) && matches(&Method::POST, PREFIX));
        assert!(matches(&Method::DELETE, "/api/antigravity-contas/trabalho"));
        assert!(matches(&Method::POST, "/api/antigravity-contas/default/login"));
        assert!(matches(&Method::POST, "/api/antigravity-contas/default/login/codigo"));
        assert!(matches(&Method::POST, "/api/antigravity-contas/default/logout"));
        assert!(!matches(&Method::DELETE, "/api/antigravity-contas/default/logout"));
        assert!(!matches(&Method::GET, "/api/antigravity-contas/a/b/c/d"));
    }

    #[test]
    fn an_account_id_is_the_folder_name_and_never_a_path() {
        let home = tempfile::tempdir().unwrap();
        assert!(account_of(home.path(), DEFAULT_ID).is_some_and(|(a, dir)| a.is_default() && dir.is_none()));
        assert!(account_of(home.path(), "trabalho").is_none(), "sem a marca não é conta");
        assert!(account_of(home.path(), "../x").is_none());
        let created = create_folder(home.path(), &json!({"name": "trabalho"}));
        assert_eq!(created.status(), StatusCode::OK);
        let (account, _) = account_of(home.path(), "trabalho").unwrap();
        assert_eq!(id_of(&account), "trabalho");
        assert_eq!(create_folder(home.path(), &json!({"name": "trabalho"})).status(), StatusCode::CONFLICT);
        assert_eq!(create_folder(home.path(), &json!({"name": "Ruim"})).status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(delete(DEFAULT_ID, None).status(), StatusCode::CONFLICT);
    }

    /// Só a conta padrão com terminal: criar conta e entrar numa adicional são recusados.
    #[test]
    fn additional_accounts_are_refused() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(create(home.path(), &json!({"name": "trabalho"})).status(), StatusCode::CONFLICT);
        assert!(!home.path().join(format!("{}trabalho", paths::ACCOUNT_PREFIX)).exists());
        assert_eq!(only_default().status(), StatusCode::CONFLICT);
    }

    mod terms {
        use super::*;
        use crate::session_open::terminal::MuxOutput;
        use futures_util::future::BoxFuture;
        use std::collections::HashSet;

        const COLORS: &str = include_str!("../../../hangar-core/tests/fixtures/agy/onboarding-colors.txt");
        const TERMS_SCREEN: &str = include_str!("../../../hangar-core/tests/fixtures/agy/terms.txt");
        const TERMS_DONE: &str = include_str!("../../../hangar-core/tests/fixtures/agy/terms-done.txt");
        const IDLE: &str = include_str!("../../../hangar-core/tests/fixtures/agy/idle.txt");
        const LOGIN: &str = include_str!("../../../hangar-core/tests/fixtures/agy/login.txt");
        const OWNER: &str = "/fixture/.gemini";

        /// O multiplexador e o `agy` falsos: a tela anda com as teclas como no `agy` de verdade, e o Done grava o aceite.
        struct World {
            sessions: HashMap<String, HashMap<String, String>>,
            screen: &'static str,
            /// A tela que o `agy` digitado abre.
            first: &'static str,
            home: PathBuf,
            created: Vec<Vec<String>>,
            typed: Vec<String>,
            keys: Vec<String>,
            /// A tela não se lê com a janela de pé.
            capture_fails: bool,
            /// `kill-session` responde 0 e não derruba (o psmux faz isso).
            kill_lies: bool,
            kills: usize,
        }

        #[derive(Clone)]
        struct Fake(Arc<Mutex<World>>);

        fn out(code: i32, stdout: &str) -> MuxOutput { MuxOutput { code: Some(code), stdout: stdout.into(), stderr: String::new() } }

        fn session_of(args: &[String]) -> String {
            let at = args.iter().position(|a| a == "-t" || a == "-s").unwrap();
            args[at + 1].trim_start_matches('=').trim_end_matches(':').to_owned()
        }

        impl World {
            fn press(&mut self, key: &str) {
                self.keys.push(key.to_owned());
                self.screen = match (self.screen, key) {
                    (COLORS, "Enter") => TERMS_SCREEN,
                    (TERMS_SCREEN, "Down") => TERMS_DONE,
                    (TERMS_DONE, "Enter") => {
                        let cache = Account::default().data_dir(&self.home).join("cache");
                        std::fs::create_dir_all(&cache).unwrap();
                        std::fs::write(cache.join("onboarding.json"), r#"{"onboardingComplete": true}"#).unwrap();
                        IDLE
                    }
                    (screen, _) => screen,
                };
            }
        }

        impl super::super::super::private_window::WindowMux for Fake {
            fn scope(&self) -> BoxFuture<'_, Vec<String>> { Box::pin(async { Vec::new() }) }
            fn run(&self, _scope: Vec<String>, args: Vec<String>, _stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput> {
                let mut world = self.0.lock().unwrap();
                let reply = match args[0].as_str() {
                    "new-session" => { world.sessions.insert(session_of(&args), HashMap::new()); world.created.push(args.clone()); out(0, "") }
                    "set-option" => {
                        let name = session_of(&args);
                        world.sessions.get_mut(&name).unwrap().insert(args[3].clone(), args[4].clone());
                        out(0, "")
                    }
                    "show-options" => match world.sessions.get(&session_of(&args)).and_then(|o| o.get(&args[4])) {
                        Some(value) => out(0, &format!("{value}\n")),
                        None => out(1, ""),
                    },
                    "has-session" => out(if world.sessions.contains_key(&session_of(&args)) { 0 } else { 1 }, ""),
                    "kill-session" => {
                        world.kills += 1;
                        if !world.kill_lies { world.sessions.remove(&session_of(&args)); }
                        out(0, "")
                    }
                    "list-sessions" => out(0, &world.sessions.keys().map(|n| format!("{n}\n")).collect::<String>()),
                    "capture-pane" if world.sessions.contains_key(&session_of(&args)) && !world.capture_fails => out(0, world.screen),
                    "capture-pane" => out(1, ""),
                    "send-keys" if args.contains(&"-l".to_owned()) => {
                        let text = args.last().unwrap().clone();
                        if text == "\r" { world.press("Enter") } else {
                            if text.ends_with("agy") { world.screen = world.first; }
                            world.typed.push(text);
                        }
                        out(0, "")
                    }
                    "send-keys" => { let key = args.last().unwrap().clone(); world.press(&key); out(0, "") }
                    _ => out(0, ""),
                };
                Box::pin(async move { reply })
            }
        }

        impl Fake {
            fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T { read(&self.0.lock().unwrap()) }
            fn windows(&self) -> Arc<PrivateWindows> {
                let env = crate::session_open::terminal::TerminalEnv { windows: false, program: "tmux".into(), display: Vec::new(),
                    server_config_dir: None, default_config_dir: PathBuf::from("/fixture/.claude"), birth: Duration::ZERO };
                Arc::new(PrivateWindows::new(Box::new(self.clone()), &env, TERMS_PREFIX, OWNER.into(), None))
            }
        }

        fn setup(first: &'static str) -> (tempfile::TempDir, Fake, TermsAttempts) {
            let home = tempfile::tempdir().unwrap();
            let world = World { sessions: HashMap::new(), screen: "", first, home: home.path().into(), created: Vec::new(),
                typed: Vec::new(), keys: Vec::new(), capture_fails: false, kill_lies: false, kills: 0 };
            (home, Fake(Arc::new(Mutex::new(world))), TermsAttempts::default())
        }

        fn mark(owner: &str) -> HashMap<String, String> {
            HashMap::from([("@cp_hidden".to_owned(), "1".to_owned()), ("@cp_login_owner".to_owned(), owner.to_owned())])
        }

        fn agy_env() -> ProcessEnv { hangar_antigravity::env::process_env(&Account::default(), []) }

        async fn reply_of(response: Response) -> (StatusCode, Value) {
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
        }

        async fn start(attempts: &TermsAttempts, fake: &Fake, home: &Path) -> String {
            let (status, body) = reply_of(start_terms(attempts, fake.windows(), home, agy_env()).await).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["estado"], "iniciando");
            body["attempt_id"].as_str().unwrap().to_owned()
        }

        async fn reach(attempts: &TermsAttempts, id: &str, want: impl Fn(&Stage) -> bool) {
            reach_within(attempts, id, Duration::from_secs(30), want).await;
        }

        /// O relógio do tokio está parado: o teto só corre enquanto a tarefa espera a próxima volta.
        async fn reach_within(attempts: &TermsAttempts, id: &str, limit: Duration, want: impl Fn(&Stage) -> bool) -> Stage {
            let mut stage = attempts.watch(id).expect("tentativa sem etapa");
            tokio::time::timeout(limit, stage.wait_for(|stage| want(stage))).await.expect("a etapa não chegou").unwrap().clone()
        }

        fn failed(stage: &Stage) -> bool { matches!(stage, Stage::Failed(_)) }

        async fn status(attempts: &TermsAttempts, id: &str) -> (StatusCode, Value) { reply_of(terms_status(attempts, Some(id)).await).await }

        async fn answer(attempts: &TermsAttempts, body: Value) -> (StatusCode, Value) { reply_of(answer_terms(attempts, &body).await).await }

        #[tokio::test(start_paused = true)]
        async fn terms_start_shows_starting_then_the_terms_text_from_a_hidden_wide_window() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            assert_eq!(status(&attempts, &id).await.1["estado"], "iniciando");
            reach(&attempts, &id, |stage| matches!(stage, Stage::Terms { .. })).await;
            let (code, body) = status(&attempts, &id).await;
            assert_eq!(code, StatusCode::OK);
            assert_eq!(body["estado"], "termos");
            let text = body["texto"].as_str().unwrap();
            assert!(text.contains("https://antigravity.google/terms") && text.contains("https://policies.google.com/privacy"), "{text}");
            let name = format!("{TERMS_PREFIX}{id}");
            let created = fake.world(|w| w.created[0].clone());
            let width: u16 = created[created.iter().position(|a| a == "-x").unwrap() + 1].parse().unwrap();
            assert!(width >= 200, "{created:?}");
            assert_eq!(created[created.iter().position(|a| a == "-c").unwrap() + 1], home.path().to_string_lossy());
            assert!(created.contains(&"AGY_CLI_DISABLE_AUTO_UPDATE=1".to_owned()), "{created:?}");
            let options = fake.world(|w| w.sessions[&name].clone());
            assert_eq!((options["@cp_hidden"].as_str(), options["@cp_login_owner"].as_str()), ("1", OWNER));
            let typed = fake.world(|w| w.typed.clone());
            assert!(typed.len() == 1 && typed[0].starts_with("env -u ") && typed[0].contains("-u SSH_CONNECTION") && typed[0].ends_with(" agy"), "{typed:?}");
            assert_eq!(fake.world(|w| w.keys.clone()), ["Enter"], "só a tela de cores ganha tecla antes da pessoa");
        }

        #[tokio::test(start_paused = true)]
        async fn terms_answer_1_presses_the_keys_until_accepted_then_closes_the_window() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            reach(&attempts, &id, |stage| matches!(stage, Stage::Terms { .. })).await;
            assert_eq!(answer(&attempts, json!({"attempt_id": id, "opcao": 3})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(answer(&attempts, json!({"attempt_id": id})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
            let (code, body) = answer(&attempts, json!({"attempt_id": id, "opcao": 1})).await;
            assert_eq!((code, &body), (StatusCode::OK, &json!({"ok": true})));
            assert_eq!(status(&attempts, &id).await.1["estado"], "respondendo");
            reach(&attempts, &id, |stage| *stage == Stage::Done).await;
            assert_eq!(status(&attempts, &id).await.1["estado"], "concluido");
            assert_eq!(fake.world(|w| w.keys.clone()), ["Enter", "Down", "Enter"]);
            assert!(fake.world(|w| w.sessions.is_empty()), "a janela fecha no fim");
        }

        /// Os clientes param de ler no login: a janela não fica aberta esperando quem não vai mais olhar.
        #[tokio::test(start_paused = true)]
        async fn terms_login_screen_ends_the_attempt_and_closes_the_window() {
            let (home, fake, attempts) = setup(LOGIN);
            let id = start(&attempts, &fake, home.path()).await;
            reach(&attempts, &id, |stage| *stage == Stage::Login).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert_eq!(status(&attempts, &id).await.1["estado"], "login");
            assert!(fake.world(|w| w.sessions.is_empty()), "a janela fecha no login");
        }

        #[tokio::test(start_paused = true)]
        async fn terms_answer_outside_the_terms_screen_is_409_and_presses_nothing() {
            let (home, fake, attempts) = setup(IDLE);
            let id = start(&attempts, &fake, home.path()).await;
            let (code, body) = answer(&attempts, json!({"attempt_id": id, "opcao": 1})).await;
            assert_eq!(code, StatusCode::CONFLICT);
            assert_eq!(body["detail"]["code"], "erro_termos_fora_da_tela", "{body}");
            assert!(fake.world(|w| w.keys.is_empty()));
        }

        #[tokio::test(start_paused = true)]
        async fn terms_unknown_attempt_is_404() {
            let (home, fake, attempts) = setup(IDLE);
            let id = start(&attempts, &fake, home.path()).await;
            for (code, body) in [status(&attempts, "outra").await, answer(&attempts, json!({"attempt_id": "outra", "opcao": 1})).await] {
                assert_eq!(code, StatusCode::NOT_FOUND);
                assert_eq!(body["detail"]["code"], "erro_tentativa_inexistente", "{body}");
            }
            assert_eq!(reply_of(terms_status(&attempts, None).await).await.0, StatusCode::NOT_FOUND);
            assert_eq!(status(&attempts, &id).await.0, StatusCode::OK);
        }

        #[tokio::test(start_paused = true)]
        async fn terms_delete_closes_the_window_and_forgets_the_attempt() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), Some("outra")).await).await;
            assert_eq!((code, body), (StatusCode::OK, json!({"ok": true})));
            assert_eq!(status(&attempts, &id).await.0, StatusCode::OK, "DELETE de outra tentativa não toca nesta");
            assert_eq!(reply_of(cancel_terms(&attempts, &fake.windows(), Some(&id)).await).await.0, StatusCode::OK);
            assert_eq!(fake.world(|w| w.created.len()), 1);
            assert!(fake.world(|w| w.sessions.is_empty()), "DELETE fecha a janela");
            assert_eq!(status(&attempts, &id).await.0, StatusCode::NOT_FOUND);
        }

        #[tokio::test(start_paused = true)]
        async fn terms_a_new_attempt_closes_the_previous_window() {
            let (home, fake, attempts) = setup(COLORS);
            let first = start(&attempts, &fake, home.path()).await;
            reach(&attempts, &first, |stage| matches!(stage, Stage::Terms { .. })).await;
            let second = start(&attempts, &fake, home.path()).await;
            assert!(!fake.world(|w| w.sessions.contains_key(&format!("{TERMS_PREFIX}{first}"))), "a anterior fecha");
            assert_eq!(status(&attempts, &first).await.0, StatusCode::NOT_FOUND);
            assert_eq!(status(&attempts, &second).await.1["estado"], "iniciando");
        }

        #[tokio::test(start_paused = true)]
        async fn terms_window_gone_on_the_terms_screen_fails_and_a_later_answer_is_409() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            reach(&attempts, &id, |stage| matches!(stage, Stage::Terms { .. })).await;
            fake.0.lock().unwrap().sessions.clear();
            reach(&attempts, &id, failed).await;
            let (code, body) = answer(&attempts, json!({"attempt_id": id, "opcao": 1})).await;
            assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_termos_fora_da_tela")), "{body}");
            let (_, body) = status(&attempts, &id).await;
            assert_eq!((body["estado"].as_str(), body["codigo"].as_str()), (Some("falhou"), Some(TERMINAL_FAILED)), "{body}");
            assert_eq!(fake.world(|w| w.keys.clone()), ["Enter"], "nada apertado depois do fim");
        }

        #[tokio::test(start_paused = true)]
        async fn terms_capture_that_keeps_failing_still_runs_the_60_s_limit() {
            let (home, fake, attempts) = setup(COLORS);
            fake.0.lock().unwrap().capture_fails = true;
            let id = start(&attempts, &fake, home.path()).await;
            let stage = reach_within(&attempts, &id, Duration::from_secs(120), failed).await;
            assert_eq!(stage, Stage::Failed("termos_prazo"));
        }

        #[tokio::test(start_paused = true)]
        async fn terms_ten_minutes_on_the_terms_screen_expire() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            reach(&attempts, &id, |stage| matches!(stage, Stage::Terms { .. })).await;
            let stage = reach_within(&attempts, &id, Duration::from_secs(11 * 60), failed).await;
            assert_eq!(stage, Stage::Failed("termos_expirou"));
            assert!(fake.world(|w| w.sessions.is_empty()));
        }

        #[tokio::test(start_paused = true)]
        async fn terms_delete_retries_a_kill_that_lies_and_reports_the_open_window() {
            let (home, fake, attempts) = setup(COLORS);
            let id = start(&attempts, &fake, home.path()).await;
            fake.0.lock().unwrap().kill_lies = true;
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), Some(&id)).await).await;
            assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("erro_termos_janela_aberta")), "{body}");
            assert!(fake.world(|w| w.kills) >= 2, "o kill que não derrubou é repetido");
            // A tentativa já saiu do mapa: o segundo Cancelar tenta de novo em vez de responder ok.
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), Some(&id)).await).await;
            assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("erro_termos_janela_aberta")), "{body}");
            fake.0.lock().unwrap().kill_lies = false;
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), Some(&id)).await).await;
            assert_eq!((code, body), (StatusCode::OK, json!({"ok": true})));
            assert!(fake.world(|w| w.sessions.is_empty()));
        }

        #[tokio::test(start_paused = true)]
        async fn terms_delete_of_an_unknown_attempt_sweeps_and_reports_a_window_still_open() {
            let (_home, fake, attempts) = setup(IDLE);
            {
                let mut world = fake.0.lock().unwrap();
                world.sessions.insert(format!("{TERMS_PREFIX}velha"), mark(OWNER));
                world.kill_lies = true;
            }
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), Some("velha")).await).await;
            assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("erro_termos_janela_aberta")), "{body}");
            fake.0.lock().unwrap().kill_lies = false;
            let (code, body) = reply_of(cancel_terms(&attempts, &fake.windows(), None).await).await;
            assert_eq!((code, body), (StatusCode::OK, json!({"ok": true})));
            assert!(fake.world(|w| w.sessions.is_empty()), "a janela que sobrou fecha");
        }

        #[tokio::test(start_paused = true)]
        async fn terms_sweep_closes_only_hidden_orphans_of_this_server() {
            let (_home, fake, _) = setup(IDLE);
            {
                let mut world = fake.0.lock().unwrap();
                world.sessions.insert(format!("{TERMS_PREFIX}a"), mark(OWNER));
                world.sessions.insert(format!("{TERMS_PREFIX}b"), mark("/other/.gemini"));
                world.sessions.insert("term-login-0123456789abcdef0123456789abcdef".into(), mark(OWNER));
            }
            fake.windows().sweep(|_| true).await;
            let left: HashSet<String> = fake.world(|w| w.sessions.keys().cloned().collect());
            assert_eq!(left, HashSet::from([format!("{TERMS_PREFIX}b"), "term-login-0123456789abcdef0123456789abcdef".to_owned()]));
        }

        fn state(home: &Path) -> Arc<crate::routes::AppState> {
            let mut state = crate::routes::AppState::new(crate::config::Config {
                listen: "127.0.0.1:0".parse().unwrap(), internal_secret: "fixture-secret".into(), auth_token: "fixture-owner".into(),
                log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
            });
            let empty = home.join("bin");
            std::fs::create_dir_all(&empty).unwrap();
            state.accounts = crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::from_map([
                ("HOME".to_owned(), home.to_string_lossy().into_owned()), ("USERPROFILE".to_owned(), home.to_string_lossy().into_owned()),
                ("PATH".to_owned(), empty.to_string_lossy().into_owned()),
            ].into()));
            Arc::new(state)
        }

        async fn route(state: &Arc<crate::routes::AppState>, method: Method, uri: &str) -> (StatusCode, Value) {
            let request = Request::builder().method(method).uri(uri).body(axum::body::Body::empty()).unwrap();
            reply_of(public(state.clone(), request).await).await
        }

        #[tokio::test]
        async fn terms_routes_refuse_an_additional_account_and_a_missing_agy() {
            let home = tempfile::tempdir().unwrap();
            assert_eq!(create_folder(home.path(), &json!({"name": "trabalho"})).status(), StatusCode::OK);
            let state = state(home.path());
            for (method, uri) in [(Method::POST, "/api/antigravity-contas/trabalho/termos"), (Method::GET, "/api/antigravity-contas/trabalho/termos?attempt_id=x"),
                (Method::POST, "/api/antigravity-contas/trabalho/termos/resposta"), (Method::DELETE, "/api/antigravity-contas/trabalho/termos")] {
                assert!(matches(&method, uri.split('?').next().unwrap()), "{uri}");
                let (code, body) = route(&state, method, uri).await;
                assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some(ADDITIONAL)), "{uri}");
            }
            let (code, body) = route(&state, Method::POST, "/api/antigravity-contas/default/termos").await;
            assert_eq!((code, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_agy_ausente")));
        }
    }
}
