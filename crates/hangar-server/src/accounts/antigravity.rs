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
use hangar_antigravity::{login, paths, probe, Account};
use serde_json::{Value, json};

use super::catalog::{AccountError, valid_name};
use super::http::{Json, error};

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
        [id, "login", "codigo"] | [id, "logout"] => !id.is_empty() && *method == Method::POST,
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
    for check in futures_util::future::join_all(checks).await {
        let Ok((account, signed, in_use)) = check else { continue };
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
    let value = probe::signed_in(program, home, &env, PROBE_LIMIT).ok().flatten();
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

fn create(home: &Path, body: &Value) -> Response {
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
                if ok && let Ok(command) = crate::provider_hooks::command(home, "agy-state-hook")
                    && let Err(e) = hangar_antigravity::hooks::install(&account.hooks_path(home), &command) {
                    tracing::warn!(code = "agy_hooks_after_login_failed", error = %e, "hook do Antigravity não gravado depois do login");
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

async fn logout(program: &Path, home: &Path, env: hangar_antigravity::env::ProcessEnv, account: &Account) -> Response {
    let (program, cwd) = (program.to_owned(), home.to_owned());
    let done = tokio::task::spawn_blocking(move || probe::logout(&program, &cwd, &env, PROBE_LIMIT)).await;
    forget_signed(account, home);
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
        let created = create(home.path(), &json!({"name": "trabalho"}));
        assert_eq!(created.status(), StatusCode::OK);
        let (account, _) = account_of(home.path(), "trabalho").unwrap();
        assert_eq!(id_of(&account), "trabalho");
        assert_eq!(create(home.path(), &json!({"name": "trabalho"})).status(), StatusCode::CONFLICT);
        assert_eq!(create(home.path(), &json!({"name": "Ruim"})).status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(delete(DEFAULT_ID, None).status(), StatusCode::CONFLICT);
    }
}
