//! O preparo da conta pela API do servidor, com o andamento no pane. `GET` reaproveita a conta pronta e
//! só confia a pasta nova; outro estado pede o preparo (`POST`), e a espera só consulta. Com o servidor
//! fora, a conta já preparada abre do jeito que está.
use std::path::Path;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;
use tokio::time::Instant;

use super::Screen;
use crate::accounts::preparation::PREPARED_MARKER;

/// Quanto esperar o preparo em andamento; depois, a sessão não abre.
const WAIT: Duration = Duration::from_secs(90);
/// Intervalo entre as consultas: a API não tem stream.
const POLL: Duration = Duration::from_millis(250);
/// Prazo de cada pedido.
const REQUEST: Duration = Duration::from_secs(5);
/// O `quote(..., safe='')`: só letras, números e `-._~` ficam.
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Onde o servidor atende e com que token. O token não chega ao pane (é segredo do servidor): sem ele
/// no ambiente, vem do `.env`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Server { pub token: Option<String>, pub port: String }

pub(crate) struct Target<'a> {
    pub account: &'a str,
    pub cwd: &'a str,
    pub codex_home: &'a Path,
    pub server: Server,
}

/// `CP_AUTH_TOKEN`/`CP_PORT` do ambiente vencem o `.env` dos dados do usuário; porta padrão 8765.
pub(crate) fn server_of(env: &[(String, String)], home: &Path) -> Server {
    let backend = crate::session_open::cli::backend_dir_for_home(home);
    let pick = |key: &str| super::value_of(env, key).filter(|value| !value.is_empty()).map(str::to_owned)
        .or_else(|| hangar_core::user_data::env_value(home, backend.as_deref(), key).ok().flatten().filter(|value| !value.is_empty()));
    Server { token: pick("CP_AUTH_TOKEN"), port: pick("CP_PORT").unwrap_or_else(|| "8765".into()) }
}

/// Por que um pedido não voltou com o estado.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// Sem conexão, prazo ou sem token: o servidor está fora para o lançador.
    Down(String),
    /// Respondeu com erro, ou com algo que não é o estado.
    Failed(String),
}

fn path(account: &str, cwd: &str) -> String {
    format!("/api/codex-contas/{}/prepare?cwd={}", utf8_percent_encode(account, QUOTE), utf8_percent_encode(cwd, QUOTE))
}

async fn ask(server: &Server, method: reqwest::Method, path: &str) -> Result<serde_json::Map<String, Value>, Asked> {
    let token = server.token.as_deref().ok_or_else(|| Asked::Down("CP_AUTH_TOKEN ausente".into()))?;
    let port: u16 = server.port.trim().parse().map_err(|_| Asked::Failed(format!("CP_PORT inválida: {:?}", server.port)))?;
    let client = crate::http_client().timeout(REQUEST).build().map_err(|error| Asked::Failed(error.to_string()))?;
    let url = format!("http://127.0.0.1:{port}{path}");
    let response = match client.request(method, &url).bearer_auth(token).send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() => return Err(Asked::Down(format!("sem conexão com 127.0.0.1:{port}"))),
        Err(error) if error.is_timeout() => return Err(Asked::Down(format!("127.0.0.1:{port} não respondeu em 5 s"))),
        Err(_) => return Err(Asked::Failed(format!("pedido a 127.0.0.1:{port} falhou"))),
    };
    let status = response.status();
    if !status.is_success() {
        return Err(Asked::Failed(format!("HTTP {}", status.as_u16())));
    }
    let body = response.bytes().await.map_err(|_| Asked::Failed("resposta do preparo interrompida".into()))?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(result)) => Ok(result),
        _ => Err(Asked::Failed("resposta do preparo não é o estado da conta".into())),
    }
}

fn reason(asked: Asked) -> String { match asked { Asked::Down(reason) | Asked::Failed(reason) => reason } }

/// O texto de `{codigo, params, texto}`; outra forma vira texto, e o vazio é nenhum.
fn stage_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(message)) => message.get("texto").and_then(Value::as_str).unwrap_or("").to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if number.as_f64() != Some(0.0) => number.to_string(),
        Some(Value::Bool(true)) => "True".into(),
        _ => String::new(),
    }
}

/// `status` ou, sem a chave, `estado`.
fn state_of(result: &serde_json::Map<String, Value>) -> Option<&str> {
    match result.get("status") {
        Some(status) => status.as_str(),
        None => result.get("estado").and_then(Value::as_str),
    }
}

/// Os códigos de pendência que vão à tela: só `[a-z0-9_]+`, nunca os parâmetros.
fn issues(result: &serde_json::Map<String, Value>) -> Vec<String> {
    let valid = |code: &str| !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    result.get("issues").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|issue| issue.get("code").and_then(Value::as_str).filter(|code| valid(code)).map(str::to_owned)).collect()
}

/// Prepara (ou confere) a conta; `Err` é o motivo de não abrir.
pub(crate) async fn run(target: &Target<'_>, screen: &dyn Screen) -> Result<(), String> {
    let path = path(target.account, target.cwd);
    let first = match ask(&target.server, reqwest::Method::GET, &path).await {
        Ok(result) => result,
        Err(Asked::Down(reason)) => return offline(target, reason, screen),
        Err(failed) => return Err(self::reason(failed)),
    };
    // Parcial também pede o preparo: o servidor reaproveita o recente e tenta de novo o antigo.
    let result = if matches!(first.get("status").and_then(Value::as_str), Some("ready" | "running")) {
        first
    } else {
        ask(&target.server, reqwest::Method::POST, &path).await.map_err(reason)?
    };
    wait(result, screen, || ask(&target.server, reqwest::Method::GET, &path)).await
}

/// Servidor fora: só a conta com a marca do preparo abre, com a pasta pré-confiada aqui.
fn offline(target: &Target<'_>, reason: String, screen: &dyn Screen) -> Result<(), String> {
    let marker = target.codex_home.join(PREPARED_MARKER);
    if !marker.is_file() {
        return Err(reason);
    }
    crate::session_open::codex::pretrust(target.codex_home, target.cwd);
    screen.say("serviço indisponível; reutilizando a conta já preparada");
    let partial = std::fs::read(&marker).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|marker| marker.get("status").and_then(Value::as_str) == Some("partial"));
    if partial {
        screen.say("a conta foi preparada com pendências; confira a sincronização da conta");
    }
    Ok(())
}

/// Espera o preparo em andamento sem iniciar outro; a linha só sai quando a etapa muda (o pane mudo
/// era indistinguível de um travado).
async fn wait<F, Fut>(mut result: serde_json::Map<String, Value>, screen: &dyn Screen, consult: F) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Map<String, Value>, Asked>>,
{
    let deadline = Instant::now() + WAIT;
    let mut previous: Option<String> = None;
    loop {
        let state = state_of(&result);
        if matches!(state, Some("ready" | "ok")) {
            return Ok(());
        }
        if !matches!(state, Some("running" | "executando")) {
            for code in issues(&result) {
                screen.say(&format!("pendência do preparo: {code}"));
            }
            // Parcial: um recurso não sincronizou (plugin fora do marketplace): abre e avisa.
            if state == Some("partial") {
                screen.say("a sessão abre com pendências; confira a sincronização da conta");
                return Ok(());
            }
            return Err("o preparo não terminou; confira as pendências da sincronização antes de abrir".into());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err("a conta continua em preparo; aguarde a conclusão e tente abrir novamente".into());
        }
        let stage = Some(stage_text(result.get("etapa"))).filter(|stage| !stage.is_empty()).unwrap_or_else(|| "preparando a conta".into());
        if previous.as_deref() != Some(stage.as_str()) {
            screen.say(&format!("{stage}…"));
            previous = Some(stage);
        }
        tokio::time::sleep(POLL.min(deadline - now)).await;
        result = consult().await.map_err(reason)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Lines(Mutex<Vec<String>>);
    impl Screen for Lines {
        fn say(&self, line: &str) { self.0.lock().unwrap().push(line.to_owned()); }
    }
    impl Lines {
        fn new() -> Self { Self(Mutex::default()) }
        fn all(&self) -> Vec<String> { self.0.lock().unwrap().clone() }
    }

    /// Um servidor falso: responde cada pedido com o próximo da fila e grava método, caminho e token.
    async fn server(replies: Vec<(u16, Value)>) -> (Server, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut replies = replies.into_iter();
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).await.unwrap_or(0) == 1 { head.push(byte[0]); }
                let head = String::from_utf8_lossy(&head).into_owned();
                let first = head.lines().next().unwrap_or("").to_owned();
                let auth = head.lines().find_map(|line| line.strip_prefix("authorization: ").or_else(|| line.strip_prefix("Authorization: ")))
                    .unwrap_or("").to_owned();
                log.lock().unwrap().push(format!("{} {}", first.rsplit_once(' ').map_or(first.as_str(), |(request, _)| request), auth));
                let (status, body) = replies.next().unwrap_or((500, json!({})));
                let body = body.to_string();
                let reply = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (Server { token: Some("fake-owner-token".into()), port }, seen)
    }

    fn target<'a>(home: &'a Path, server: Server) -> Target<'a> {
        Target { account: "x", cwd: "/repo com espaço é #%", codex_home: home, server }
    }

    const PATH: &str = "/api/codex-contas/x/prepare?cwd=%2Frepo%20com%20espa%C3%A7o%20%C3%A9%20%23%25";

    /// `launcher__prepare-running-then-ready`: uma linha por etapa nova, só consultas depois do pedido.
    #[tokio::test]
    async fn a_running_preparation_is_waited_with_one_line_per_stage() {
        let home = tempfile::tempdir().unwrap();
        let (server, seen) = server(vec![
            (200, json!({"status": "running", "etapa": {"codigo": "skills", "params": {}, "texto": "copiando skills"}})),
            (200, json!({"status": "running", "etapa": {"codigo": "skills", "params": {}, "texto": "copiando skills"}})),
            (200, json!({"status": "running", "etapa": {"codigo": "plugins", "params": {"segredo": 1}, "texto": "instalando plugins"}})),
            (200, json!({"status": "running", "etapa": null})),
            (200, json!({"status": "ready"})),
        ]).await;
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), server), &lines).await, Ok(()));
        assert_eq!(lines.all(), ["copiando skills…", "instalando plugins…", "preparando a conta…"]);
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().all(|line| line == &format!("GET {PATH} Bearer fake-owner-token")), "{seen:?}");
        assert_eq!(seen.len(), 5);
    }

    /// `launcher__prepare-idle-post` e `-partial` e `-error`: só o estado pronto ou em andamento dispensa o pedido.
    #[tokio::test]
    async fn another_state_asks_for_the_preparation_and_partial_opens_with_the_codes() {
        let home = tempfile::tempdir().unwrap();
        let (server, seen) = server(vec![(200, json!({"status": "idle"})), (200, json!({"status": "running", "etapa": "preparando"})),
            (200, json!({"status": "ok"}))]).await;
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), server), &lines).await, Ok(()));
        assert_eq!(lines.all(), ["preparando…"]);
        let methods: Vec<String> = seen.lock().unwrap().iter().map(|line| line.split(' ').next().unwrap().to_owned()).collect();
        assert_eq!(methods, ["GET", "POST", "GET"]);

        let (server, _) = server(vec![(200, json!({"status": "partial"})), (200, json!({"status": "partial", "issues": [
            {"code": "plugin_fora_do_marketplace", "params": {"segredo": "x"}}, {"code": "skill_x"}, {"code": "Inválido!"}, "texto", {"code": 3}]}))]).await;
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), server), &lines).await, Ok(()));
        assert_eq!(lines.all(), ["pendência do preparo: plugin_fora_do_marketplace", "pendência do preparo: skill_x",
            "a sessão abre com pendências; confira a sincronização da conta"]);

        let (server, _) = server(vec![(200, json!({"status": "idle"})), (200, json!({"status": "error", "issues": [{"code": "login_ausente"}]}))]).await;
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), server), &lines).await,
            Err("o preparo não terminou; confira as pendências da sincronização antes de abrir".into()));
        assert_eq!(lines.all(), ["pendência do preparo: login_ausente"]);

        // Erro HTTP não é servidor fora: não reaproveita a marca.
        std::fs::write(home.path().join(PREPARED_MARKER), "{}").unwrap();
        let (server, _) = server(vec![(500, json!({}))]).await;
        assert_eq!(run(&target(home.path(), server), &Lines::new()).await, Err("HTTP 500".into()));
    }

    /// `launcher__prepare-server-down-*`: com a marca abre e pré-confia a pasta; sem ela, não abre.
    #[tokio::test]
    async fn a_prepared_account_opens_with_the_server_down() {
        let home = tempfile::tempdir().unwrap();
        let down = Server { token: Some("t".into()), port: free_port() };
        assert_eq!(run(&target(home.path(), down.clone()), &Lines::new()).await, Err(format!("sem conexão com 127.0.0.1:{}", down.port)));
        assert_eq!(run(&target(home.path(), Server { token: None, port: "1".into() }), &Lines::new()).await, Err("CP_AUTH_TOKEN ausente".into()));
        std::fs::write(home.path().join(PREPARED_MARKER), json!({"version": 1, "status": "partial"}).to_string()).unwrap();
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), down), &lines).await, Ok(()));
        assert_eq!(lines.all(), ["serviço indisponível; reutilizando a conta já preparada",
            "a conta foi preparada com pendências; confira a sincronização da conta"]);
        assert_eq!(std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            "\n[projects.\"/repo com espaço é #%\"]\ntrust_level = \"trusted\"\n\n");
        let lines = Lines::new();
        assert_eq!(run(&target(home.path(), Server { token: None, port: "1".into() }), &lines).await, Ok(()), "sem token também é servidor fora");
        assert_eq!(lines.all().len(), 2);
    }

    fn free_port() -> String { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string() }

    /// O prazo de 90 s da espera, sem esperar 90 s: o relógio do tokio pausado.
    #[tokio::test(start_paused = true)]
    async fn a_preparation_that_never_ends_gives_up_after_ninety_seconds() {
        let consults = Arc::new(Mutex::new(0usize));
        let counter = consults.clone();
        let running = || json!({"status": "running"}).as_object().unwrap().clone();
        let lines = Lines::new();
        let result = wait(running(), &lines, || {
            *counter.lock().unwrap() += 1;
            async move { Ok(json!({"status": "running"}).as_object().unwrap().clone()) }
        }).await;
        assert_eq!(result, Err("a conta continua em preparo; aguarde a conclusão e tente abrir novamente".into()));
        assert_eq!(lines.all(), ["preparando a conta…"]);
        assert_eq!(*consults.lock().unwrap(), 360, "uma consulta a cada 0,25 s");
    }

    proptest::proptest! {
        /// Qualquer resposta do servidor chega a um desfecho, e a tela só leva código no formato da regra.
        #[test]
        fn any_answer_has_an_outcome_and_only_clean_codes_reach_the_screen(body in proptest_json()) {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
            let Value::Object(first) = body.clone() else { return Ok(()) };
            let lines = Lines::new();
            let _ = runtime.block_on(wait(first, &lines, || {
                let body = body.clone();
                async move { match body { Value::Object(map) => Ok(map), _ => Err(Asked::Failed("x".into())) } }
            }));
            for line in lines.all() {
                if let Some(code) = line.strip_prefix("pendência do preparo: ") {
                    proptest::prop_assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "{line}");
                }
            }
        }
    }

    fn proptest_json() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![Just(Value::Null), any::<bool>().prop_map(Value::from), any::<i64>().prop_map(Value::from),
            "[a-zA-Z0-9_ !é]{0,8}".prop_map(Value::from),
            prop_oneof![Just("ready"), Just("running"), Just("partial"), Just("error"), Just("ok"), Just("executando")].prop_map(Value::from)];
        leaf.prop_recursive(3, 24, 4, |inner| prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            proptest::collection::btree_map(prop_oneof![Just("status".to_owned()), Just("estado".to_owned()), Just("issues".to_owned()),
                Just("code".to_owned()), Just("etapa".to_owned()), Just("texto".to_owned()), "[a-z]{1,4}".prop_map(String::from)], inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ])
    }
}
