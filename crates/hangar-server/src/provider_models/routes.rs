//! Fronteira HTTP; não há chamada de volta ao Python para descobrir sessão, catálogo ou estado.
use super::{
    ModelError,
    driver::{self, Io, IoFuture, Timing},
    pure,
};
use crate::{list::mux::Pane, routes::AppState};
use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use hangar_api::session::SessionRow;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, UNIX_EPOCH},
};

fn query(raw: Option<&str>) -> HashMap<String, String> {
    form_urlencoded::parse(raw.unwrap_or("").as_bytes())
        .into_owned()
        .collect()
}
fn route(path: &str) -> Option<(String, &'static str, bool)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (name, rest) = rest.split_once('/')?;
    let (provider, write) = match rest {
        "pi/models" => ("pi", false),
        "pi/model" => ("pi", true),
        "kimi/models" => ("kimi", false),
        "kimi/model" => ("kimi", true),
        _ => return None,
    };
    let name = percent_encoding::percent_decode_str(name)
        .decode_utf8()
        .ok()?
        .into_owned();
    Some((name, provider, write))
}
pub fn matches(method: &Method, path: &str) -> bool {
    if path == "/api/pi/inbox" {
        return *method == Method::GET;
    }
    route(path).is_some_and(|(_, _, write)| {
        if write {
            *method == Method::POST
        } else {
            *method == Method::GET
        }
    })
}
pub fn catalog_request(method: &Method, path: &str, raw: Option<&str>) -> bool {
    *method == Method::GET
        && path == "/api/model-options"
        && query(raw)
            .get("provider")
            .is_some_and(|p| ["pi", "omp", "kimi"].contains(&p.as_str()))
}
pub fn inbox_http_request(method: &Method, path: &str, headers: &HeaderMap) -> bool {
    path == "/api/pi/inbox"
        && (*method != Method::GET
            || !headers
                .get("upgrade")
                .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket")))
}
pub fn allowed_method(path: &str, raw: Option<&str>) -> Option<Method> {
    if catalog_request(&Method::GET, path, raw) {
        return Some(Method::GET);
    }
    route(path).map(|(_, _, write)| if write { Method::POST } else { Method::GET })
}
fn coded(status: u16, code: &str, message: &str) -> ModelError {
    ModelError::coded(status, code, message, json!({}))
}
fn catalog_error(provider: &str) -> ModelError {
    if provider == "kimi" {
        return coded(
            409,
            "erro_catalogo_kimi_indisponivel",
            "catalogo do Kimi indisponível — ~/.kimi-code/config.toml ausente ou sem seções [models.*]",
        );
    }
    if provider == "omp" {
        coded(
            409,
            "erro_catalogo_omp_indisponivel",
            "catalogo do omp indisponivel — rode ./scripts/install-claude-wrapper.sh e feche e reabra a sessao (o /reload do omp nao recarrega a extensao)",
        )
    } else {
        coded(
            409,
            "erro_catalogo_pi_indisponivel",
            "catalogo do Pi indisponivel — rode ./scripts/install-claude-wrapper.sh e reinicie a sessao (extensao hangar-state.ts desatualizada)",
        )
    }
}
async fn discovered(
    st: &Arc<AppState>,
    name: &str,
) -> Result<(SessionRow, Option<u32>), ModelError> {
    st.list
        .discovered_session(name)
        .await
        .map_err(|e| coded(503, e.code, "não consegui descobrir a sessão"))?
        .ok_or_else(|| coded(404, "erro_sessao_inexistente", "sessão não encontrada"))
}
fn same_life(a: &SessionRow, b: &SessionRow) -> bool {
    a.name == b.name
        && a.provider == b.provider
        && a.jsonl == b.jsonl
        && a.lifecycle_id == b.lifecycle_id
}
fn validate_kind(row: &SessionRow, provider: &str) -> Result<(), ModelError> {
    if provider == "pi" {
        if row.jsonl.as_deref().is_none_or(str::is_empty) {
            return Err(coded(
                404,
                "erro_sessao_inexistente",
                "sessao ou transcript nao encontrado",
            ));
        }
        if !["pi", "omp"].contains(&row.provider.as_str()) {
            return Err(coded(
                400,
                "erro_rota_so_pi",
                "esta rota so existe pra sessoes Pi",
            ));
        }
    } else if row.provider != "kimi" {
        return Err(coded(
            400,
            "erro_rota_so_kimi",
            "esta rota só existe pra sessões Kimi",
        ));
    }
    Ok(())
}
async fn config_dir(st: &Arc<AppState>, pid: Option<u32>) -> PathBuf {
    let env = st.list.env().clone();
    let default = env
        .dirs
        .as_ref()
        .map(|d| d.home.join(".claude"))
        .unwrap_or_else(|| st.origins_home.join(".claude"));
    tokio::task::spawn_blocking(move||pid.and_then(|pid|match env.procs.env_var(pid.into(),"CLAUDE_CONFIG_DIR"){Ok(value)=>value.filter(|s|!s.is_empty()).map(PathBuf::from),Err(error)=>{tracing::warn!(code="provider_config_dir_unavailable",io_kind=?error.kind(),"não consegui resolver a conta do pane; usando a padrão");None}}).unwrap_or(default)).await.unwrap_or_else(|_|st.origins_home.join(".claude"))
}
async fn session_catalog(
    st: &Arc<AppState>,
    row: &SessionRow,
    pid: Option<u32>,
) -> Result<(Value, PathBuf), ModelError> {
    let config = config_dir(st, pid).await;
    let c = config.clone();
    let jsonl = row.jsonl.clone().unwrap_or_default();
    let value = tokio::task::spawn_blocking(move || pure::pi_catalog(&jsonl, &c))
        .await
        .ok()
        .flatten()
        .ok_or_else(|| catalog_error(&row.provider))?;
    Ok((value, config))
}
async fn global_catalog(
    st: &Arc<AppState>,
    raw: Option<String>,
    catalogs: &Arc<super::catalog::Catalogs>,
) -> Result<Value, ModelError> {
    let query = query(raw.as_deref());
    let provider = query.get("provider").map(String::as_str).unwrap_or("");
    if query.get("engine_account").is_some() {
        return Err(coded(
            400,
            "erro_cliproxy_conta",
            "conta ChatGPT exige Claude com motor CLIProxyAPI local",
        ));
    }
    if provider == "kimi" {
        let home = st.origins_home.clone();
        let cat = tokio::task::spawn_blocking(move || pure::kimi_catalog(&home))
            .await
            .ok()
            .flatten()
            .ok_or_else(|| catalog_error("kimi"))?;
        return Ok(
            json!({"kind":"kimi","reduced":false,"models":cat["models"],"default":cat["default"]}),
        );
    }
    Ok(json!({"kind":provider,"reduced":false,"models":catalogs.read(provider).await?}))
}

/// A borda do servidor já decidiu quem pode usar a rota; o WebSocket confere ainda self-connect/token.
pub async fn serve(st: &Arc<AppState>, request: Request) -> Response {
    serve_with_catalogs(st, request, super::catalog::CATALOGS.clone()).await
}
pub(super) async fn serve_with_catalogs(
    st: &Arc<AppState>,
    request: Request,
    catalogs: Arc<super::catalog::Catalogs>,
) -> Response {
    if request.uri().path() == "/api/pi/inbox" {
        if inbox_http_request(request.method(), request.uri().path(), request.headers()) {
            return crate::session_write::json_response(
                StatusCode::NOT_FOUND,
                json!({"detail":"Not Found"}),
            );
        }
        let Some(peer) = request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|v| v.0)
        else {
            return coded(
                403,
                "pi_inbox_peer_missing",
                "origem da extensão indisponível",
            )
            .response();
        };
        return super::inbox::websocket(st, peer, request).await;
    }
    let result = if request.uri().path() == "/api/model-options" {
        global_catalog(st, request.uri().query().map(str::to_owned), &catalogs).await
    } else {
        session(st, request).await
    };
    match result {
        Ok(value) => crate::session_write::json_response(StatusCode::OK, value),
        Err(error) => error.response(),
    }
}
async fn session(st: &Arc<AppState>, request: Request) -> Result<Value, ModelError> {
    let (name, provider, write) =
        route(request.uri().path()).ok_or_else(|| ModelError::text(404, "rota não encontrada"))?;
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !write {
        let (row, pid) = discovered(st, &name).await?;
        validate_kind(&row, provider)?;
        if provider == "pi" {
            return Ok(pure::pi_response(&session_catalog(st, &row, pid).await?.0));
        }
        let home = st.origins_home.clone();
        return tokio::task::spawn_blocking(move || pure::kimi_catalog(&home))
            .await
            .ok()
            .flatten()
            .ok_or_else(|| catalog_error("kimi"));
    }
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| ModelError::text(413, "corpo grande demais"))?;
    let body = parse_http_body(&bytes, provider, content_type.as_deref())?;
    let (row, _pid) = discovered(st, &name).await?;
    validate_kind(&row, provider)?;
    let home = st.origins_home.clone();
    let lock_name = name.clone();
    let guard =
        tokio::task::spawn_blocking(move || {
            crate::terminal_lock::acquire(&home, &lock_name, std::time::Duration::from_secs(10))
        })
            .await
            .map_err(|_| {
                coded(
                    503,
                    "provider_keyboard_lock_failed",
                    "não consegui reservar o teclado",
                )
            })?
            .map_err(|e| if e.kind() == std::io::ErrorKind::WouldBlock {
                coded(409, "provider_keyboard_busy", "o teclado da sessão seguiu ocupado; tente de novo")
            } else {
                coded(
                    503,
                    "provider_keyboard_lock_failed",
                    "não consegui reservar o teclado",
                )
            })?;
    // A operação destacada conserva a trava mesmo se o HTTP desconectar durante o redraw ou read-back.
    let st = st.clone();
    tokio::spawn(async move{
  let _guard=guard;st.list.invalidate();let(current,pid)=discovered(&st,&name).await?;
  if !same_life(&row,&current){return Err(coded(409,"erro_interacao_desatualizada","a sessão mudou enquanto esperava o teclado"));}
  if current.transfer_id.is_some(){return Err(coded(409,"erro_transferencia_em_andamento","a sessão está sendo transferida"));}
  if provider=="kimi"&&st.term.is_active(&name){return Err(coded(409,"erro_terminal_aberto","Terminal aberto nesta sessao. Feche o painel pra responder por aqui."));}
  let io=NativeIo::new(st.clone(),current.clone(),pid).await?;let timing=Timing::default();
  if provider=="pi"{
   let (catalog,_)=session_catalog(&st,&current,pid).await?;let mut commands=vec![];
   let model=body["model"].as_str().filter(|v|!v.is_empty());let selected=body["provider"].as_str().filter(|v|!v.is_empty());let effort=body["effort"].as_str().filter(|v|!v.is_empty());
   if let Some(model)=model{let provider=selected.ok_or_else(||ModelError::text(422,"provider obrigatorio junto com model"))?;pure::check_pi(&catalog,provider,model)?;commands.push(pure::model_command(provider,model)?);}
   if let Some(effort)=effort{commands.push(pure::think_command(effort)?);}
   if commands.is_empty(){return Err(coded(422,"erro_model_effort_faltando","informe model (com provider) e/ou effort"));}
   driver::prepare(&io,timing).await?;driver::pi_commands(&io,&commands,timing).await?;
   driver::pi_read_back(&io,&catalog,selected,model,effort,timing).await
  }else{
   let cdir=io.config.clone();let jsonl=current.jsonl.clone();let env=st.list.env().clone();
   let working=tokio::task::spawn_blocking(move||{
      let dirs=env.dirs.as_ref().map(|d|vec![cdir.clone(),d.claude.clone(),d.default_claude()]).unwrap_or_else(||vec![cdir]);
      kimi_working(&dirs,jsonl.as_deref())
   }).await.map_err(|_|coded(503,"provider_state_unavailable","não consegui conferir o estado da sessão"))?;
   if working{return Err(coded(409,"erro_sessao_trabalhando","a sessão está trabalhando — espere ela terminar"));}
   let home=st.origins_home.clone();let catalog=tokio::task::spawn_blocking(move||pure::kimi_catalog(&home)).await.ok().flatten().ok_or_else(||catalog_error("kimi"))?;
   let target=body["model"].as_str().filter(|v|!v.is_empty()).map(|m|pure::check_kimi(&catalog,m)).transpose()?;
   let effort=body["effort"].as_str().filter(|v|!v.is_empty()).map(|e|if let Some(target)=target.as_ref(){pure::check_effort(target,e)}else{pure::clean(e,"alias").map(|v|v.to_lowercase())}).transpose()?;
   if target.is_none()&&effort.is_none(){return Err(coded(422,"erro_model_effort_faltando","informe model e/ou effort"));}
   driver::prepare(&io,timing).await?;let result=driver::kimi_switch(&io,target.as_ref().and_then(|t|t["alias"].as_str()),target.as_ref().and_then(|t|t["name"].as_str()),effort.as_deref(),timing).await?;
   Ok(json!({"ok":true,"current":target.as_ref().map(|t|json!({"alias":t["alias"],"name":t["name"]})),"effort":effort,"result":result["result"]}))
  }
 }).await.map_err(|_|coded(503,"provider_model_driver_failed","o controle de modelo falhou"))?
}
pub(super) fn parse_http_body(
    bytes: &[u8],
    provider: &str,
    content_type: Option<&str>,
) -> Result<Value, ModelError> {
    let mime = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if bytes.is_empty()
        || mime == "application/json"
        || mime.starts_with("application/") && mime.ends_with("+json")
    {
        return parse_body(bytes, provider);
    }
    Err(ModelError {
        status: 422,
        detail: json!([{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":String::from_utf8_lossy(bytes)}]),
    })
}
pub fn parse_body(bytes: &[u8], provider: &str) -> Result<Value, ModelError> {
    let missing = || ModelError {
        status: 422,
        detail: json!([{"type":"missing","loc":["body"],"msg":"Field required","input":null}]),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(missing());
    }
    let value:Value=std::str::from_utf8(bytes).ok().and_then(crate::transcript::pyjson::loads_lossless)
        .map(Ok).unwrap_or_else(||serde_json::from_slice(bytes)).map_err(|error| {
        let text=String::from_utf8_lossy(bytes);
        let reason=error.to_string();
        let message=if reason.contains("key must be a string") || (error.is_eof() && text.trim_end().ends_with('{')) {
            "Expecting property name enclosed in double quotes"
        } else if reason.contains("expected `:`") { "Expecting ':' delimiter" }
        else if reason.contains("trailing characters") { "Extra data" }
        else if reason.contains("expected `,` or `}`") || reason.contains("expected `,` or `]`") { "Expecting ',' delimiter" }
        else { "Expecting value" };
        let position=if error.is_eof(){text.chars().count()}else{
            let before=bytes.split(|b|*b==b'\n').take(error.line().saturating_sub(1)).map(|line|line.len()+1).sum::<usize>();
            let index=(before+error.column().saturating_sub(1)).min(bytes.len());
            String::from_utf8_lossy(&bytes[..index]).chars().count()
        };
        ModelError { status:422, detail:json!([{"type":"json_invalid","loc":["body",position],"msg":"JSON decode error","input":{},"ctx":{"error":message}}]) }
    })?;
    if value.is_null() {
        return Err(missing());
    }
    let Some(fields) = value.as_object() else {
        return Err(ModelError {
            status: 422,
            detail: json!([{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":value}]),
        });
    };
    let allowed = if provider == "pi" {
        vec!["provider", "model", "effort"]
    } else {
        vec!["model", "effort"]
    };
    let mut errors = vec![];
    let mut normalized = serde_json::Map::new();
    for key in &allowed {
        let input = fields.get(*key).unwrap_or(&Value::Null);
        if !input.is_string() && !input.is_null() {
            errors.push(json!({"type":"string_type","loc":["body",key],"msg":"Input should be a valid string","input":input}));
        }
        normalized.insert((*key).to_owned(), input.clone());
    }
    for (key, input) in fields {
        if !allowed.contains(&key.as_str()) {
            errors.push(json!({"type":"extra_forbidden","loc":["body",key],"msg":"Extra inputs are not permitted","input":input}));
        }
    }
    if errors.is_empty() {
        Ok(Value::Object(normalized))
    } else {
        Err(ModelError {
            status: 422,
            detail: json!(errors),
        })
    }
}

struct NativeIo {
    st: Arc<AppState>,
    row: SessionRow,
    pane: Pane,
    target: String,
    config: PathBuf,
}
impl NativeIo {
    async fn new(st: Arc<AppState>, row: SessionRow, pid: Option<u32>) -> Result<Self, ModelError> {
        let panes = st
            .list
            .env()
            .mux
            .list_panes()
            .await
            .map_err(|e| coded(503, e.code, "multiplexador indisponível"))?;
        let target = st
            .list
            .agent_target(&row.name)
            .await
            .map_err(|e| coded(503, e.code, "pane do agente indisponível"))?;
        let pane = panes
            .iter()
            .find(|p| {
                p.session == row.name
                    && target
                        .as_ref()
                        .is_some_and(|t| p.target().as_ref() == Some(t))
            })
            .or_else(|| panes.iter().find(|p| p.session == row.name && p.active))
            .cloned()
            .ok_or_else(|| {
                ModelError::text(
                    409,
                    "pane com overlay aberto ou sessao morta — nada foi digitado",
                )
            })?;
        let target = pane
            .target()
            .unwrap_or_else(|| format!("={}:{}", row.name, pane.window_index.unwrap_or(0)));
        let config = config_dir(&st, pid).await;
        Ok(Self {
            st,
            row,
            pane,
            target,
            config,
        })
    }
    async fn validate(&self) -> Result<(), ModelError> {
        let panes = self
            .st
            .list
            .env()
            .mux
            .list_panes()
            .await
            .map_err(|e| coded(503, e.code, "multiplexador indisponível"))?;
        if panes.iter().any(|p| {
            p.session == self.pane.session
                && p.pane_id == self.pane.pane_id
                && p.pid == self.pane.pid
                && p.session_created == self.pane.session_created
                && !p.dead
        }) {
            Ok(())
        } else {
            Err(coded(
                409,
                "erro_interacao_desatualizada",
                "o pane da sessão mudou; nenhuma tecla foi enviada à nova vida",
            ))
        }
    }
    fn mux(&self) -> crate::state::capture::MuxProcess {
        crate::state::capture::MuxProcess::new(
            self.st.list.env().capture_program.clone(),
            Duration::from_secs(5),
        )
    }
}
impl Io for NativeIo {
    fn capture(&self) -> IoFuture<'_, Result<String, ModelError>> {
        Box::pin(async move {
            self.validate().await?;
            self.mux()
                .capture_checked(&self.row.name, &self.target)
                .await
                .map_err(|code| coded(503, code, "não consegui ler o terminal da sessão"))
        })
    }
    fn key<'a>(&'a self, key: &'a str, literal: bool) -> IoFuture<'a, Result<(), ModelError>> {
        Box::pin(async move {
            self.validate().await?;
            let mut args = vec!["send-keys", "-t", self.target.as_str()];
            let windows_escape = "\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_";
            if literal {
                args.extend(["-l", "--", key]);
            } else if key == "Enter" && !cfg!(windows) {
                args.extend(["-l", "--", "\r"]);
            } else if matches!(key, "Escape" | "Esc") && cfg!(windows) {
                args.extend(["-l", "--", windows_escape]);
            } else {
                args.push(key);
            }
            let output = self
                .mux()
                .run(&args)
                .await
                .map_err(|code| coded(503, code, "não consegui enviar a tecla"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(coded(
                    503,
                    "provider_key_refused",
                    "o multiplexador recusou a tecla; confira o terminal",
                ))
            }
        })
    }
    fn editor(&self) -> IoFuture<'_, Option<String>> {
        Box::pin(async move {
            let key =
                super::inbox::INBOX.line_for(Some(&self.row.name), Some(&self.pane.pane_id))?;
            super::inbox::INBOX.question(&key, "editor").await
        })
    }
    fn catalog(&self) -> IoFuture<'_, Option<Value>> {
        Box::pin(async move {
            if self.validate().await.is_err() {
                return None;
            }
            let jsonl = self.row.jsonl.clone()?;
            let config = self.config.clone();
            tokio::task::spawn_blocking(move || pure::pi_catalog(&jsonl, &config))
                .await
                .ok()
                .flatten()
        })
    }
}

pub fn kimi_turn_open(path: &Path) -> Option<bool> {
    let result = (|| -> std::io::Result<Option<bool>> {
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        let start = size.saturating_sub(4 << 20);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![];
        file.read_to_end(&mut bytes)?;
        let lines: Vec<_> = bytes.split(|b| *b == b'\n').collect();
        for (index, line) in lines.iter().enumerate().rev() {
            if start > 0 && index == 0 {
                continue;
            }
            let Some(value) = std::str::from_utf8(line).ok().and_then(crate::transcript::pyjson::loads_lossless) else {
                continue;
            };
            match value["type"].as_str() {
                Some("turn.prompt" | "turn.steer") => return Ok(Some(true)),
                Some("turn.ended" | "turn.cancel") => return Ok(Some(false)),
                _ => {}
            }
        }
        Ok(None)
    })();
    match result {
        Ok(Some(value)) => Some(value),
        _ => {
            if crate::warn_limit::allow(path.to_str(), "provider_kimi_turn_unknown") {
                tracing::warn!(
                    code = "provider_kimi_turn_unknown",
                    "wire do Kimi sem fronteira legível; mantém a reserva por tempo"
                );
            }
            None
        }
    }
}
fn modified(path: &Path) -> Option<f64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|v| v.as_secs_f64())
}
static KIMI_MARKERS: LazyLock<Mutex<HashMap<Vec<PathBuf>, crate::list::facts_files::HookStates>>> =
    LazyLock::new(Mutex::default);
pub fn kimi_working(config_dirs: &[PathBuf], jsonl: Option<&str>) -> bool {
    let Some(jsonl) = jsonl.filter(|j| !j.is_empty()) else {
        return false;
    };
    let mut caches = KIMI_MARKERS.lock().unwrap_or_else(|e| e.into_inner());
    let markers = caches.entry(config_dirs.to_vec()).or_default();
    markers.refresh(config_dirs);
    let marker = markers.get_state(Some(&crate::locate::session_key(jsonl)), |_| false);
    let Some(marker) = marker else {
        return false;
    };
    if marker.state != "idle" {
        return marker.state == "working";
    }
    match kimi_turn_open(Path::new(jsonl)) {
        Some(value) => value,
        None => {
            let path = Path::new(jsonl);
            let agents = path.parent().and_then(Path::parent);
            let mut latest = modified(path);
            if let Some(agents) = agents.filter(|p| p.file_name().is_some_and(|n| n == "agents")) {
                if let Ok(entries) = std::fs::read_dir(agents) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            if let Some(value) = modified(&entry.path().join("wire.jsonl")) {
                                latest = Some(latest.unwrap_or(0.).max(value));
                            }
                        }
                    }
                }
            }
            latest.is_some_and(|mtime| mtime > marker.ts + 2.)
        }
    }
}
