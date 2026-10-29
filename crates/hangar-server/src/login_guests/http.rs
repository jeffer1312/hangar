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
            bootstrap:String::new(), secret:store::token().into_bytes(), secure:false,
            rate_max:10.into(), rate_window:300.into() }
    }
    pub fn from_config(home: &std::path::Path, claude: &std::path::Path, backend: Option<&std::path::Path>) -> super::store::Result<Self> {
        let read = |key| crate::machine_peers::config_value(home,backend,key).map_err(|_| config_invalid());
        let mut settings=Self::defaults(home,claude);
        if let Some(value)=read("CP_SYNC")? {
            settings.sync_enabled=validation::boolean(&json!({"sync":value}),"sync",false).map_err(|_|config_invalid())?;
        }
        if let Some(value)=read("CP_SYNC_DATA")? { settings.vault=PathBuf::from(value); }
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
    pub rate_now: fn() -> f64,
    pub(crate) lives: std::sync::OnceLock<Arc<crate::share::life::Lives>>,
    pub(super) sync: Sync,
    pub now: fn() -> u64,
    pub entropy: fn(usize) -> Vec<u8>,
    pub id: fn() -> String,
    pub token: fn() -> String,
}
impl Service {
    pub fn from_env() -> Self {
        let env=crate::accounts::environment::AccountEnvironment::capture();
        Self::from_config(&env.home,&env.claude_base,crate::session_open::cli::backend_dir().as_deref())
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
                Ok(bytes) => crate::orchestration::body::parse(&bytes),
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
                let mut r = Json(value).into_response();
                if let Some(cookie) = cookie {
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
        crate::routes::cors(&headers, response.headers_mut());
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
                .map(|model| super::schema::validate(model, body))
                .transpose()?;
            let body = normalized.as_ref().unwrap_or(body);
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
                let g = self.sync.generation(u);
                set_cookie =
                    Some(self.cookie_header(&self.sync.cookie(u, g.as_deref(), now), https));
            } else if authenticated {
                set_cookie = Some(self.cookie_header(
                    &self.sync.cookie(
                        user.as_deref().unwrap_or_default(),
                        generation.as_deref(),
                        now,
                    ),
                    https,
                ));
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
            .map(|model| super::schema::validate(model, body))
            .transpose()?;
        let body = normalized.as_ref().unwrap_or(body);
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
    fn cookie_header(&self, cookie: &str, https: bool) -> String {
        format!(
            "cp_sync={cookie}; HttpOnly; Max-Age=2592000; Path=/; SameSite=lax{}",
            if https || self.sync.secure {
                "; Secure"
            } else {
                ""
            }
        )
    }
}
fn cookie(headers: &HeaderMap, key: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|v| {
            let (k, v) = v.trim().split_once('=')?;
            (k == key).then(|| v.trim_matches('"').to_owned())
        })
}
fn reply(f: Failure, headers: &HeaderMap) -> Response {
    let mut r = (
        axum::http::StatusCode::from_u16(f.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        Json(f.body),
    )
        .into_response();
    crate::routes::cors(headers, r.headers_mut());
    r
}
fn declared_too_large(headers: &HeaderMap, limit: usize) -> bool {
    let Some(raw) = headers.get(header::CONTENT_LENGTH).and_then(|value| value.to_str().ok()) else { return false };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) { return false; }
    let digits = raw.trim_start_matches('0');
    let limit = limit.to_string();
    digits.len() > limit.len() || digits.len() == limit.len() && digits > limit.as_str()
}
fn too_large() -> Response {
    (axum::http::StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response()
}
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
    let response = st
        .login_guests
        .respond(owner, forward.client_ip, forward.https, request)
        .await;
    if response.status().is_success() {
        st.list.invalidate();
    }
    response
}

struct Json(Value);
impl IntoResponse for Json {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            self.0.to_string(),
        )
            .into_response()
    }
}
