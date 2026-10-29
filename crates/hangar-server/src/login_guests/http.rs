use super::{
    store::{self, Failure, Store},
    sync::Sync,
    validation,
};
use crate::routes::AppState;
use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

static PROCESS_SECRET:std::sync::LazyLock<Vec<u8>>=std::sync::LazyLock::new(||store::token().into_bytes());

pub struct Settings {
    pub guests: PathBuf,
    pub vault: PathBuf,
    pub runtime_config: PathBuf,
    pub sync_enabled: bool,
    pub bootstrap: String,
    pub secret: Vec<u8>,
    pub secure: bool,
    pub rate_max: num_bigint::BigInt,
    pub rate_window: num_bigint::BigInt,
}
impl Settings {
    fn defaults(home: &std::path::Path, claude: &std::path::Path) -> Self {
        Self { guests:claude.join("guests.json"), vault:home.join(".hangar/sync-vault.json"),
            runtime_config:claude.join("runtime-config.json"), sync_enabled:false,
            bootstrap:String::new(), secret:Vec::new(), secure:false,
            rate_max:10.into(), rate_window:300.into() }
    }
    pub fn from_config(home: &std::path::Path, claude: &std::path::Path, backend: Option<&std::path::Path>) -> super::store::Result<Self> {
        let snapshot=crate::machine_peers::dotenv::SettingsSnapshot::capture(home,backend,
            crate::machine_peers::dotenv::SettingsEnvironment::capture()).map_err(|_|config_invalid())?;
        let mut settings=Self::from_snapshot(home,claude,&snapshot)?;
        if settings.secret.is_empty() { settings.secret=PROCESS_SECRET.clone(); }
        Ok(settings)
    }
    pub fn from_snapshot(home:&std::path::Path,claude:&std::path::Path,snapshot:&crate::machine_peers::dotenv::SettingsSnapshot)->super::store::Result<Self> {
        let read=|key|snapshot.value(key).map_err(|_|config_invalid());
        let mut settings=Self::defaults(home,claude);
        if let Some(value)=read("CP_SYNC")? {
            settings.sync_enabled=validation::boolean(&json!({"sync":value}),"sync",false).map_err(|_|config_invalid())?;
        }
        if let Some(value)=read("CP_SYNC_DATA")? { settings.vault=crate::value_coercion::path(&value); }
        if let Some(value)=read("CP_SYNC_BOOTSTRAP")? { settings.bootstrap=value; }
        if let Some(value)=read("CP_SYNC_SESSION_SECRET")?.filter(|v|!v.is_empty()) { settings.secret=value.into_bytes(); }
        if let Some(value)=read("CP_LAN_BIND_IP")? {
            settings.secure=!["127.0.0.1","localhost","::1","0.0.0.0","auto"].contains(&value.as_str());
        }
        for (key,target) in [("CP_SYNC_RATE_MAX",&mut settings.rate_max),("CP_SYNC_RATE_WINDOW",&mut settings.rate_window)] {
            if let Some(value)=read(key)? {
                *target=crate::value_coercion::integer(&Value::String(value)).and_then(|v|v.to_string().parse().ok()).ok_or_else(config_invalid)?;
            }
        }
        Ok(settings)
    }
}
fn config_invalid() -> Failure {
    Failure::code(503,"sync_config_invalid","configuração de sincronização inválida")
}

pub struct Service {
    pub store: Arc<Store>,
    pub body_limit: usize,
    config_error: Option<Failure>,
    pub(crate) needs_initialization: bool,
    pub rate_now: fn() -> f64,
    pub(crate) lives: std::sync::OnceLock<Arc<crate::share::life::Lives>>,
    pub(super) sync: Sync,
    pub now: fn() -> u64,
    pub entropy: fn(usize) -> Vec<u8>,
    pub id: fn() -> String,
    pub token: fn() -> String,
}
impl Service {
    pub async fn from_state(state: &AppState) -> Self {
        let home=&state.accounts.env.home;let claude=&state.accounts.env.claude_base;
        let settings=state.settings_snapshot().map_err(|_|config_invalid())
            .and_then(|snapshot|Settings::from_snapshot(home,claude,&snapshot));
        let mut settings=match settings {
            Ok(settings)=>settings,
            Err(error)=>{let mut service=Self::new(Settings::defaults(home,claude));service.config_error=Some(error);return service;}
        };
        if settings.secret.is_empty() {
            settings.secret=match state.auth.sync_session_key().await {
                Ok(Some(secret))=>secret,
                Ok(None)=>PROCESS_SECRET.clone(),
                Err(_)=>{let mut service=Self::new(settings);service.config_error=Some(Failure::code(503,"sync_session_context_unavailable","contexto de autenticação da sincronização indisponível"));return service;}
            };
        }
        Self::new(settings)
    }
    pub fn uninitialized(home:&std::path::Path,claude:&std::path::Path)->Self {
        let mut service=Self::new(Settings::defaults(home,claude));
        service.config_error=Some(config_invalid());service.needs_initialization=true;service
    }
    pub fn from_config(home: &std::path::Path, claude: &std::path::Path, backend: Option<&std::path::Path>) -> Self {
        match Settings::from_config(home,claude,backend) {
            Ok(settings)=>Self::new(settings),
            Err(error)=>{ let mut service=Self::new(Settings::defaults(home,claude)); service.config_error=Some(error); service }
        }
    }
    pub fn new(settings: Settings) -> Self {
        let mut sync = Sync::new(settings.vault, settings.runtime_config, settings.secret);
        sync.bootstrap = settings.bootstrap;
        sync.enabled = settings.sync_enabled;
        sync.secure = settings.secure;
        sync.rate_max = settings.rate_max;
        sync.rate_window = settings.rate_window;
        Self {
            store: Store::shared(settings.guests),
            body_limit: 100 * 1024 * 1024,
            config_error: None,
            needs_initialization:false,
            rate_now: || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64()),
            lives: std::sync::OnceLock::new(),
            sync,
            now: || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            },
            entropy: store::random,
            id: store::id,
            token: store::token,
        }
    }
    /// A mesma função atende a porta pública e as provas HTTP, sem depender do Python.
    pub async fn respond(
        self: &Arc<Self>,
        owner: bool,
        ip: String,
        https: bool,
        request: Request,
    ) -> Response {
        if declared_too_large(request.headers(), self.body_limit) {
            return too_large();
        }
        let (parts, body) = request.into_parts();
        let headers = parts.headers.clone();
        let method = parts.method.clone();
        let path = percent_encoding::percent_decode_str(parts.uri.path())
            .decode_utf8_lossy()
            .into_owned();
        let decision=crate::http_contract::resolve(ROUTES,method.as_str(),&path);
        if let Some(mut response)=crate::http_route_response::response(&decision,&method,&parts.uri,&headers,https,"localhost") {
            crate::http_cors::apply(&headers,&mut response);
            return response;
        }
        if path.starts_with("/api/sync/") && let Some(error)=self.config_error.clone() {
            return reply(error,&headers);
        }
        let query = parts.uri.query().map(str::to_owned);
        let expects_body = match (method.as_str(), path.as_str()) {
            ("POST", "/api/guests" | "/api/sync/setup" | "/api/sync/register" | "/api/sync/login" | "/api/sync/guests") => true,
            ("PUT", "/api/sync/vault") => true,
            ("POST", path) => path.strip_prefix("/api/guests/").is_some_and(|id| !id.contains('/')),
            _ => false,
        };
        let parsed = if expects_body {
            match to_bytes(body, self.body_limit).await {
                Ok(bytes) if bytes.is_empty() => Ok(Value::Null),
                Ok(bytes) if json_content_type(&parts.headers) => crate::orchestration::body::parse(&bytes),
                Ok(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(text) => Ok(Value::String(text.into())),
                    Err(_) => return serialization_failure(&headers),
                },
                Err(_) => Err((axum::http::StatusCode::BAD_REQUEST, json!({"detail":"There was an error parsing the body"}))),
            }
        } else { Ok(Value::Null) };
        let body = match parsed {
            Ok(value) => value,
            Err((status, body)) => {
                return reply(
                    Failure {
                        status: status.as_u16(),
                        body,
                    },
                    &headers,
                );
            }
        };
        let body=super::Tree(body);
        let service = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            service.dispatch(
                owner,
                &ip,
                https,
                &method,
                &path,
                query.as_deref(),
                &parts.headers,
                &body,
            )
        })
        .await;
        let mut response = match result {
            Ok(Ok((value, cookie))) => {
                let mut r = Json(crate::orchestration::codec::normalize_response(value)).into_response();
                if r.status().is_success() && let Some(cookie) = cookie {
                    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
                        r.headers_mut().insert(header::SET_COOKIE, cookie);
                    }
                }
                r
            }
            Ok(Err(e)) => reply(e, &headers),
            Err(_) => reply(
                Failure::code(
                    500,
                    "guests_worker_failed",
                    "operação de convidados interrompida",
                ),
                &headers,
            ),
        };
        crate::http_cors::apply(&headers, &mut response);
        response
    }
    fn dispatch(
        &self,
        owner: bool,
        ip: &str,
        https: bool,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &Value,
    ) -> super::store::Result<(Value, Option<String>)> {
        let now = (self.now)();
        if let Some(path) = path.strip_prefix("/api/sync/") {
            let setup = path == "setup" || path == "setup/disable";
            if setup && !owner {
                return Err(super::sync::unauthorized());
            }
            if !setup && !self.sync.enabled() {
                return Err(Failure {
                    status: 404,
                    body: json!({"detail":"Not Found"}),
                });
            }
            let mut user = None;
            let mut generation = None;
            let authenticated = path == "vault" || path == "guests" || path.starts_with("guests/");
            if authenticated {
                if method != Method::GET
                    && method != Method::HEAD
                    && headers
                        .get("sec-fetch-site")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v == "same-site" || v == "cross-site")
                {
                    return Err(Failure::code(403, "erro_nao_autorizado", "unauthorized"));
                }
                let cookie = cookie(headers, "cp_sync").unwrap_or_default();
                let (u, g) = self.sync.authenticate(&cookie, now)?;
                user = Some(u);
                generation = g;
            }
            if path == "prelogin" {
                user = crate::auth::query_param(query, "user");
            }
            if path == "guests" && method == Method::POST {
                self.sync.owner(user.as_deref())?;
            }
            let model = match (method.as_str(), path) {
                ("POST", "setup") => Some("SetupBody"),
                ("POST", "register") => Some("RegisterBody"),
                ("POST", "login") => Some("LoginBody"),
                ("PUT", "vault") => Some("VaultPutBody"),
                ("POST", "guests") => Some("GuestAccountBody"),
                _ => None,
            };
            let normalized = model
                .map(|model| super::schema::validate(model, body).map(super::Tree))
                .transpose()?;
            let body = normalized.as_deref().unwrap_or(body);
            let value = self.sync.handle(
                method.as_str(),
                path,
                body,
                user.as_deref(),
                (self.rate_now)(),
                ip,
                self.entropy,
            )?;
            let mut set_cookie = None;
            if path == "login" && method == Method::POST {
                let u = body["user"].as_str().unwrap_or_default();
                let g = self.sync.generation(u)?;
                set_cookie =
                    Some(self.cookie_header(&self.sync.cookie(u, Some(&g), now), https)?);
            } else if authenticated {
                set_cookie = Some(self.cookie_header(
                    &self.sync.cookie(
                        user.as_deref().unwrap_or_default(),
                        generation.as_deref(),
                        now,
                    ),
                    https,
                )?);
            } else if path == "logout" && method == Method::POST {
                set_cookie = Some("cp_sync=\"\"; Max-Age=0; Path=/; SameSite=lax".into());
            }
            return Ok((value, set_cookie));
        }
        let guest = if owner {
            None
        } else {
            crate::auth::presented_token(headers, query, method, https)
                .and_then(|t| self.store.lookup(&t))
        };
        if !owner && guest.is_none() {
            return Err(super::sync::unauthorized());
        }
        if path == "/api/me" && method == Method::GET {
            return Ok((
                guest.map_or_else(
                    || json!({"role":"owner","name":null}),
                    |g| json!({"role":"guest","name":g.name}),
                ),
                None,
            ));
        }
        if !owner {
            return Err(Failure::code(
                403,
                "erro_fora_do_convidado",
                "fora do acesso do convidado",
            ));
        }
        let model = if method == Method::POST && path == "/api/guests" {
            Some("NewGuest")
        } else if method == Method::POST
            && path.starts_with("/api/guests/")
            && !path.ends_with("/delete")
        {
            Some("GuestSettings")
        } else {
            None
        };
        let normalized = model
            .map(|model| super::schema::validate(model, body).map(super::Tree))
            .transpose()?;
        let body = normalized.as_deref().unwrap_or(body);
        let value = if method == Method::POST && path == "/api/guests" {
            let (root, sees, owner) = validation::guest_settings(body)?;
            let name = validation::text(body, "name", 1, 100)?;
            self.store
                .create(&name, &root, sees, owner, (self.id)(), (self.token)())?
        } else if method == Method::POST && path.starts_with("/api/guests/") {
            let tail = &path[12..];
            if let Some(id) = tail.strip_suffix("/delete") {
                self.store.delete(id)?
            } else {
                let (root, sees, owner) = validation::guest_settings(body)?;
                self.store.update(tail, &root, sees, owner)?
            }
        } else {
            return Err(Failure {
                status: 405,
                body: json!({"detail":"Method Not Allowed"}),
            });
        };
        Ok((value, None))
    }
    fn cookie_header(&self, cookie: &str, https: bool) -> super::store::Result<String> {
        let cookie=quote_cookie(cookie)?;
        Ok(format!(
            "cp_sync={cookie}; HttpOnly; Max-Age=2592000; Path=/; SameSite=lax{}",
            if https || self.sync.secure {
                "; Secure"
            } else {
                ""
            }
        ))
    }
}
fn cookie(headers: &HeaderMap, key: &str) -> Option<String> {
    let raw=headers.get(header::COOKIE)?.as_bytes().iter().map(|b|char::from(*b)).collect::<String>();
    raw.split(';').filter_map(|part|{
        let (name,value)=part.split_once('=')?;
        (name.trim()==key).then(||unquote_cookie(value.trim()))
    }).last()
}
fn quote_cookie(value: &str) -> super::store::Result<String> {
    if value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}' || c > '\u{ff}') {
        return Err(Failure { status:500, body:Value::String("Internal Server Error".into()) });
    }
    let legal=|c:char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~:".contains(c);
    if value.chars().all(legal) { return Ok(value.into()); }
    let mut result=String::from("\"");
    for c in value.chars() {
        match c {
            '"' | '\\' => { result.push('\\'); result.push(c); }
            ',' | ';' | '\u{80}'..='\u{ff}' => result.push_str(&format!("\\{:03o}",c as u32)),
            _ => result.push(c),
        }
    }
    result.push('"'); Ok(result)
}
fn unquote_cookie(value: &str) -> String {
    let Some(value)=value.strip_prefix('"').and_then(|v|v.strip_suffix('"')) else { return value.into() };
    let mut chars=value.chars().peekable();let mut result=String::new();
    while let Some(c)=chars.next() {
        if c!='\\' { result.push(c); continue; }
        let Some(next)=chars.next() else { result.push('\\'); break; };
        let mut tail=chars.clone();
        let octal=if ('0'..='3').contains(&next) {
            match (tail.next(),tail.next()) {
                (Some(a @ '0'..='7'),Some(b @ '0'..='7'))=>Some((next as u32-'0' as u32)*64+(a as u32-'0' as u32)*8+b as u32-'0' as u32),
                _=>None,
            }
        } else {None};
        if let Some(value)=octal { chars=tail;result.push(char::from_u32(value).unwrap()); }
        else { result.push(next); }
    }
    result
}
fn serialization_failure(_headers: &HeaderMap) -> Response {
    let mut response = (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    response.extensions_mut().insert(crate::http_cors::UnhandledServerError);
    response
}
fn json_content_type(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|raw| {
        let media = raw.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        media.strip_prefix("application/").is_some_and(|kind| kind == "json" || kind.ends_with("+json"))
    })
}
fn reply(f: Failure, headers: &HeaderMap) -> Response {
    if f.status==500 && f.body.as_str()==Some("Internal Server Error") { return serialization_failure(headers); }
    let unhandled = f.body["detail"]["code"].as_str().is_some_and(|code| code.starts_with("sync_") || code=="guests_worker_failed");
    let mut r = Json(f.body).into_response();
    if r.status().is_success() {
        *r.status_mut() = axum::http::StatusCode::from_u16(f.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
    if f.status==500 && matches!(r.status().as_u16(),500) {
        // Erros de I/O do cofre eram exceções externas ao CORSMiddleware.
        if unhandled { r.extensions_mut().insert(crate::http_cors::UnhandledServerError); }
    }
    crate::http_cors::apply(headers, &mut r);
    r
}
fn declared_too_large(headers: &HeaderMap, limit: usize) -> bool {
    crate::http_body_limit::precheck_with_limit(headers,limit).is_some()
}
fn too_large() -> Response { crate::http_body_limit::rejected() }
/// O tamanho declarado é recusado antes do porteiro; o fluxo só é consumido pela rota que o lê.
pub async fn body_limit(State(st): State<Arc<AppState>>, request: Request, next: axum::middleware::Next) -> Response {
    if declared_too_large(request.headers(), st.login_guests.body_limit) { return too_large(); }
    next.run(request).await
}
pub async fn route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = match crate::routes::gate(&st, peer, &request).await { Ok(value)=>value, Err(error)=>return crate::auth::unavailable(error) };
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    let admin = path == "/api/me" || path == "/api/guests" || path.starts_with("/api/guests/")
        || path == "/api/sync/setup" || path == "/api/sync/setup/disable";
    let admission = admin.then(|| crate::routes::owner_gate(&st, peer, &request));
    let response = st
        .login_guests
        .respond(owner, forward.client_ip, forward.https, request)
        .await;
    if response.status() == axum::http::StatusCode::UNAUTHORIZED
        && let Some(admission) = admission
        && let Err(mut failure) = admission.await
    {
        for (name, value) in response.headers() {
            if name.as_str().starts_with("access-control-") { failure.headers_mut().insert(name.clone(), value.clone()); }
        }
        return failure;
    }
    if response.status().is_success() {
        st.list.invalidate();
    }
    response
}


struct Json(Value);
impl IntoResponse for Json {
    fn into_response(self) -> Response {
        if crate::orchestration::codec::contains_surrogate(&self.0) || crate::orchestration::codec::contains_nonfinite(&self.0) {
            crate::orchestration::codec::discard(self.0);
            return serialization_failure(&HeaderMap::new());
        }
        let raw=crate::transcript::pyjson::dumps_compact_unicode(&self.0);
        crate::orchestration::codec::discard(self.0);
        ([(header::CONTENT_TYPE, "application/json")],raw).into_response()
    }
}

const ROUTES:&[(&str,&str)]=&[
    ("/api/sync/status","GET"),("/api/sync/register","POST"),
    ("/api/sync/prelogin","GET"),("/api/sync/login","POST"),("/api/sync/logout","POST"),
    ("/api/sync/vault","GET"),("/api/sync/vault","PUT"),
    ("/api/sync/guests","GET"),("/api/sync/guests","POST"),("/api/sync/guests/{user}/delete","POST"),
    ("/api/sync/setup","GET"),("/api/sync/setup","POST"),("/api/sync/setup/disable","POST"),
    ("/api/guests","POST"),("/api/guests/{gid}","POST"),("/api/guests/{gid}/delete","POST"),
    ("/api/me","GET"),
];
pub fn owns_path(raw:&str)->bool {
    let path=percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    ["/api/me","/api/guests","/api/sync"].iter().any(|root|path==*root || path.strip_prefix(*root).is_some_and(|tail|tail.starts_with('/')))
}
/// CORS precede autenticação e tamanho apenas nas responsabilidades desta migração.
pub async fn public_contract(request:Request,next:axum::middleware::Next)->Response {
    if !owns_path(request.uri().path()) { return next.run(request).await; }
    if let Some(response)=crate::http_cors::preflight(request.method(),request.headers()) { return response; }
    let method=request.method().clone();let headers=request.headers().clone();
    let mut response=next.run(request).await;
    crate::http_cors::apply(&headers,&mut response);
    crate::http_route_response::suppress_head(&method,&mut response);
    response
}
