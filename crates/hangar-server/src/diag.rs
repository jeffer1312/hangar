//! Falha que o Rust atende sozinho vai ao diário do Python (`POST /internal/diag`): o
//! `hangar-server.log` não entra no arquivo que o dono exporta. Uma vez por minuto por (diário, sessão, código).
use axum::body::Body;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// Código da falha que uma rota do Rust devolveu, para o diário sem reler o corpo da resposta.
#[derive(Clone, Copy)]
pub(crate) struct FailureCode(pub &'static str);

pub(crate) fn coded(mut response: axum::response::Response, code: &'static str) -> axum::response::Response {
    response.extensions_mut().insert(FailureCode(code));
    response
}

/// O código no formato que o `/internal/diag` aceita (`[A-Za-z0-9_:]{1,64}`).
fn wire_code(code:&str) -> String {
    let code:String = code.chars().map(|c|if c.is_ascii_alphanumeric() || c == '_' || c == ':' { c } else { '_' }).take(64).collect();
    if code.is_empty() { "sem_codigo".into() } else { code }
}

/// O nome que o `/internal/diag` do Python aceita para `event`.
fn wire_event(event:&str) -> String {
    const PYTHON:[&str;5] = ["runtime.send_failed","runtime.send_uncertain","runtime.command_deferred","opcao.nao_convergiu","opcao.envio_falhou"];
    let ok = |name:&str| name.len() <= 48 && !name.is_empty() && name.bytes().all(|b|b.is_ascii_lowercase() || b == b'_');
    if PYTHON.contains(&event) || event.strip_prefix("rust.").is_some_and(ok) { return event.to_owned(); }
    let name:String = event.strip_prefix("rust.").unwrap_or(event).chars()
        .map(|c|if c.is_ascii_lowercase() { c } else { '_' }).take(48).collect();
    format!("rust.{name}")
}

#[derive(Clone)]
pub struct DiagClient { upstream:SocketAddr,secret:String,http:crate::proxy::HttpClient,
    /// O diário do próprio Rust, para `report_with`; nos testes, nenhum: não sujar o diário da máquina.
    diary:Option<std::path::PathBuf> }

impl DiagClient {
    pub fn new(upstream:SocketAddr,secret:String) -> Self {
        Self { upstream,secret,http:crate::proxy::client(),diary:if cfg!(test) { None } else { diary_dir() } }
    }

    /// `report` com o que diagnostica a falha (`detalhe`, caminho, `io_kind`, ms…) no próprio evento: o
    /// `/internal/diag` do Python só aceita os quatro campos fixos, então a linha vai ao diário do Rust.
    /// `fields`: objeto de valores curtos (texto até 300, números), nunca texto de conversa.
    /// Sem a pasta do diário do Rust (nos testes, ou sem pasta de logs), vai ao do Python sem os campos.
    pub fn report_with(&self,event:&'static str,session:&str,code:&str,reason:&'static str,fields:serde_json::Value) {
        // A chave dos testes é lida aqui, não na construção: há teste que monta o estado antes de desligar.
        if self.diary.is_none() || DIARY_OFF.load(std::sync::atomic::Ordering::Relaxed) { return self.report(event,session,code,reason); }
        if !crate::warn_limit::allow(Some(session),&format!("diag:{}:{event}:{code}",self.upstream)) { return; }
        let level = if matches!(event,"rust.groups_sweep_failed" | "rust.groups_sweep_recovered") { "aviso" } else { "erro" };
        let mut line = json!({"evento":wire_event(event),"nivel":level,"sessao":session.chars().take(128).collect::<String>(),
            "codigo":wire_code(code),"motivo":reason});
        if let (Some(line), serde_json::Value::Object(extra)) = (line.as_object_mut(), fields) { line.extend(extra); }
        record(self.diary.clone(),vec![line],"diag");
    }

    /// `event` começa com `rust.`; `reason` é frase fixa do código (por isso `'static`), nunca texto
    /// de conversa nem erro formatado, que pode ecoar valores.
    /// Falha marcada por `coded`: conflito e erro do servidor vão ao diário; entrada recusada
    /// (400, 404, 413, 422) é do usuário e só volta na resposta.
    pub(crate) fn report_response(&self,event:&'static str,scope:&str,response:&axum::response::Response,reason:&'static str) {
        let status = response.status().as_u16();
        if let Some(FailureCode(code)) = response.extensions().get::<FailureCode>().copied()
            && (status == 409 || status >= 500) {
            self.report(event,scope,code,reason);
        }
    }

    pub fn report(&self,event:&'static str,session:&str,code:&str,reason:&'static str) {
        // Por diário (o Python de destino): em produção há um só, e cada servidor de teste tem o seu.
        if !crate::warn_limit::allow(Some(session),&format!("diag:{}:{event}:{code}",self.upstream)) { return; }
        let session:String = session.chars().take(128).collect();
        let body = json!({"evento":wire_event(event),"sessao":session,"codigo":wire_code(code),"motivo":reason}).to_string();
        let (client,code) = (self.clone(),code.to_owned());
        tokio::spawn(async move {
            let request = axum::http::Request::post(format!("http://{}/internal/diag",client.upstream))
                .header("x-hangar-internal",&client.secret).header("content-type","application/json").body(Body::from(body));
            let detail = match request {
                Err(_) => "pedido inválido".into(),
                Ok(request) => match tokio::time::timeout(Duration::from_secs(5),client.http.request(request)).await {
                    Ok(Ok(response)) if response.status().is_success() => return,
                    Ok(Ok(response)) => format!("status={}",response.status().as_u16()),
                    Ok(Err(error)) => format!("connect={} {error}",error.is_connect()),
                    Err(_) => "timeout".into(),
                },
            };
            // O registro que não chegou fica inteiro no log, para não sumir.
            tracing::warn!(event,session=%session,code=%code,reason,detail=%detail,"diário do Python não recebeu a falha");
        });
    }
}

/// Os tetos por dia do `diag.py`: passado o primeiro só falha entra; no segundo, nada.
pub const DIARY_DAY_CAP: u64 = 4 << 20;
const DIARY_HARD_CAP: u64 = 8 << 20;

/// A pasta das linhas do Rust no diário exportável: `diario/servidor-rust`. O Python junta todo
/// `uso-*.jsonl` da árvore do diário (baixar, últimas linhas, poda dos 7 dias), e cada arquivo fica
/// com um escritor só: no Windows o append de dois processos no mesmo arquivo se sobrescreve.
pub fn diary_dir() -> Option<std::path::PathBuf> {
    if DIARY_OFF.load(std::sync::atomic::Ordering::Relaxed) { return None; }
    Some(crate::runtime::local_policy::log_base()?.join("diario").join("servidor-rust"))
}

/// Os testes de integração rodam o servidor no processo deles, fora do `cfg!(test)`: sem isto
/// gravariam no diário real de quem roda os testes. `diary_dir_in` fica de fora: segue o HOME do
/// ambiente da conta, que os testes já apontam para pasta temporária e leem.
static DIARY_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[doc(hidden)]
pub fn disable_diary() { DIARY_OFF.store(true, std::sync::atomic::Ordering::Relaxed) }
/// `diary_dir` pelo ambiente-base do registro.
pub fn diary_dir_in(env: &crate::accounts::environment::AccountEnvironment) -> Option<std::path::PathBuf> {
    Some(crate::runtime::local_policy::log_base_in(env)?.join("diario").join("servidor-rust"))
}

/// As linhas que o próprio Rust grava no diário exportável, no formato do `diag.registrar`: texto
/// cortado em 300, campo vazio fora, `origem` servidor. Bloqueante.
pub fn write_diary(dir: &std::path::Path, events: &[serde_json::Value], now: chrono::DateTime<chrono::Local>) -> std::io::Result<()> {
    use std::io::Write;
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("uso-{}.jsonl", now.format("%Y-%m-%d")));
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    if size >= DIARY_HARD_CAP { return Ok(()); }
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
    let mut text = String::new();
    for fields in events.iter().filter_map(serde_json::Value::as_object) {
        if size >= DIARY_DAY_CAP && fields.get("nivel").and_then(serde_json::Value::as_str) == Some("ok") { continue; }
        let mut line = serde_json::Map::from_iter([("ts".to_owned(), json!(ts))]);
        for (key, value) in fields {
            match value {
                serde_json::Value::String(text) if !text.trim().is_empty() => { line.insert(key.clone(), json!(text.trim().chars().take(300).collect::<String>())); }
                serde_json::Value::Number(_) => { line.insert(key.clone(), value.clone()); }
                _ => {}
            }
        }
        line.extend([("backend".to_owned(), json!(concat!("hangar-server ", env!("CARGO_PKG_VERSION")))),
            ("pid_backend".to_owned(), json!(std::process::id())), ("origem".to_owned(), json!("servidor"))]);
        text += &format!("{}\n", serde_json::Value::Object(line));
    }
    for (cap, what) in [(DIARY_DAY_CAP, "sucesso"), (DIARY_HARD_CAP, "tudo")] {
        if size < cap && cap <= size + text.len() as u64 {
            text += &format!("{}\n", json!({"ts": ts, "evento": "diag.teto", "nivel": "aviso", "origem": "servidor", "etapa": what,
                "detalhe": format!("teto de {cap} bytes no dia")}));
        }
    }
    if text.is_empty() { return Ok(()); }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(text.as_bytes())
}

/// `write_diary` agora; falha vira aviso no log, nunca erro para quem chama. Bloqueante.
pub fn record_blocking(dir: Option<&std::path::Path>, events: &[serde_json::Value], what: &'static str) {
    let Some(dir) = dir else { return tracing::warn!(what, "diário sem pasta de logs; linha não gravada") };
    if let Err(error) = write_diary(dir, events, chrono::Local::now()) {
        tracing::warn!(kind = ?error.kind(), what, "linha do diário não gravada");
    }
}

/// `record_blocking` numa tarefa de bloqueio; quem precisa da ordem espera o retorno.
pub fn record(dir: Option<std::path::PathBuf>, events: Vec<serde_json::Value>, what: &'static str) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || record_blocking(dir.as_deref(), &events, what))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,Mutex};

    /// O diário do Python recusa evento fora de `rust.*` (400): o resto sai com o nome inteiro sob `rust.`.
    #[test]
    fn every_event_reaches_the_python_diary_under_a_name_it_takes() {
        assert_eq!(wire_event("session.limit_check"),"rust.session_limit_check");
        assert_eq!(wire_event("runtime.registration_failed"),"rust.runtime_registration_failed");
        assert_eq!(wire_event("rust.state_deliver_failed"),"rust.state_deliver_failed");
        assert_eq!(wire_event("runtime.send_failed"),"runtime.send_failed","os que o Python já aceita ficam");
        assert_eq!(wire_code("limit_switch_continue_lifecycle-busy.x"),"limit_switch_continue_lifecycle_busy_x");
        assert_eq!((wire_code(&"a".repeat(80)).len(),wire_code("").as_str()),(64,"sem_codigo"));
    }

    /// No formato do `diag.registrar` do Python, num arquivo do dia que o export dele junta
    /// (`uso-*.jsonl` em qualquer pasta do diário): texto cortado em 300, vazio fora, `origem` servidor.
    /// Passado o teto do dia só falha entra, e a linha do teto avisa.
    #[test]
    fn the_diary_lines_are_the_python_ones_in_a_file_of_the_day() {
        use chrono::TimeZone;
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Local.with_ymd_and_hms(2026, 10, 15, 9, 30, 0).unwrap();
        let events = [json!({"evento": "sessao.criar_etapa", "nivel": "ok", "sessao": "s1", "etapa": "validar", "detalhe": ""}),
            json!({"evento": "sessao.criar.fim", "nivel": "erro", "ms": 12, "detalhe": "x".repeat(400)})];
        write_diary(dir.path(), &events, now).unwrap();
        let file = dir.path().join("uso-2026-10-15.jsonl");
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&file).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0]["evento"].as_str(), lines[0]["origem"].as_str(), lines[0]["etapa"].as_str()), (Some("sessao.criar_etapa"), Some("servidor"), Some("validar")));
        assert!(lines[0].get("detalhe").is_none(), "vazio fica fora");
        assert_eq!(lines[1]["detalhe"].as_str().unwrap().chars().count(), 300);
        assert_eq!(lines[1]["ms"], 12);
        assert_eq!(lines[0]["ts"].as_str().unwrap(), now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false));
        assert_eq!(lines[0]["pid_backend"], std::process::id());
        std::fs::write(&file, vec![b'\n'; DIARY_DAY_CAP as usize]).unwrap();
        write_diary(dir.path(), &events, now).unwrap();
        let tail: Vec<serde_json::Value> = std::fs::read_to_string(&file).unwrap().lines().filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(tail.iter().map(|l| l["evento"].as_str().unwrap()).collect::<Vec<_>>(), ["sessao.criar.fim"], "acima do teto só a falha");
    }
    use tokio::io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt,BufReader};

    type Hits = Arc<Mutex<Vec<(String,serde_json::Value)>>>;

    /// Python falso do `/internal/diag`: guarda segredo e corpo de cada registro.
    async fn fake_python() -> (SocketAddr,Hits) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits:Hits = Arc::default();
        let seen = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream,_)) = listener.accept().await else { return };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let (mut length,mut secret) = (0usize,String::new());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if line == "\r\n" { break; }
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                        if let Some(value) = lower.strip_prefix("x-hangar-internal:") { secret = value.trim().into(); }
                    }
                    // Processos desta máquina sondam portas efêmeras; sem o segredo não é o cliente.
                    if secret.is_empty() { return; }
                    let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                    seen.lock().unwrap().push((secret,serde_json::from_slice(&body).unwrap()));
                    reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await.unwrap();
                });
            }
        });
        (address,hits)
    }

    async fn wait_hits(hits:&Hits,count:usize) {
        tokio::time::timeout(Duration::from_secs(5),async {
            while hits.lock().unwrap().len() < count { tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn report_reaches_python_once_per_minute_with_the_secret() {
        let (address,hits) = fake_python().await;
        let client = DiagClient::new(address,"secret-test".into());
        client.report("rust.history_failed","diag-session","history_io","leitura do transcript falhou");
        wait_hits(&hits,1).await;
        client.report("rust.history_failed","diag-session","history_io","leitura do transcript falhou");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let hits = hits.lock().unwrap();
        assert_eq!(hits.len(),1,"repetição no mesmo minuto não sai");
        assert_eq!(hits[0].0,"secret-test");
        assert_eq!(hits[0].1,json!({"evento":"rust.history_failed","sessao":"diag-session","codigo":"history_io",
            "motivo":"leitura do transcript falhou"}));
    }

    /// O limite é por diário: outro Python (outro servidor no mesmo processo) recebe o seu registro.
    #[tokio::test]
    async fn the_limit_is_per_diary_not_per_process() {
        let (first,first_hits) = fake_python().await;
        let (second,second_hits) = fake_python().await;
        DiagClient::new(first,"secret-test".into()).report("rust.history_failed","per-diary","history_io","leitura do transcript falhou");
        wait_hits(&first_hits,1).await;
        DiagClient::new(second,"secret-test".into()).report("rust.history_failed","per-diary","history_io","leitura do transcript falhou");
        wait_hits(&second_hits,1).await;
    }
}
