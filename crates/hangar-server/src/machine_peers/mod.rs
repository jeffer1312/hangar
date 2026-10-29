//! Máquinas do dono, lista dos apps e alcance, sem passagem pelo backend Python.
pub mod cli;
pub mod doctor;
pub mod dotenv;
pub mod network;
pub mod store;
use crate::{
    routes::{AppState, owner_gate},
    session_write::{detail_body, json_response},
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Mesmo valor efetivo do servidor: ambiente vence o `.env` dos dados do usuário.
pub fn config_value(
    home: &Path,
    backend: Option<&Path>,
    key: &str,
) -> Result<Option<String>, String> {
    config_value_with_settings_environment(home,backend,key,&dotenv::SettingsEnvironment::capture())
}

/// Consulta usando a ordem original do ambiente congelado pelo serviço.
pub fn config_value_with_settings_environment(
    home:&Path,backend:Option<&Path>,key:&str,environment:&dotenv::SettingsEnvironment,
)->Result<Option<String>,String>{
    if let Some(value)=environment.value("",key)? {return Ok(Some(value));}
    let Some(path)=crate::user_data::path(home,backend,".env").map_err(|e|e.to_string())? else{return Ok(None);};
    let raw=match crate::runtime::queue::read_replaced_to_string(&path){
        Ok(raw)=>raw,
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),
        Err(error)=>return Err(error.to_string()),
    };
    environment.value(&raw,key)
}

/// Consulta com o ambiente congelado do chamador, sem ler variáveis globais durante a resolução.
pub fn config_value_with_environment(
    home: &Path, backend: Option<&Path>, key: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    if let Some((_, value)) = environment.iter().rev().find(|(name, _)| name.to_lowercase() == key.to_lowercase()) {
        return Ok(Some(value.clone()));
    }
    let Some(path) = crate::user_data::path(home, backend, ".env").map_err(|e|e.to_string())? else { return Ok(None); };
    let raw = match crate::runtime::queue::read_replaced_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    Ok(dotenv::file_settings(&raw, environment).remove(&key.to_lowercase()).flatten())
}

const ID_PUBLICATION: &str = "# hangar-server-id-publication=";

/// Uma gravação do dono supera só o override que o Supervisor ainda herdou antes dela.
pub fn startup_identity(home: &Path, backend: Option<&Path>) -> Result<String, String> {
    let inherited = std::env::var("CP_SERVER_ID").ok();
    resolve_identity(home, backend, inherited.as_deref())
}
pub fn resolve_identity(home: &Path, backend: Option<&Path>, inherited: Option<&str>) -> Result<String, String> {
    let Some(path) = crate::user_data::path(home, backend, ".env").map_err(|e| e.to_string())? else {
        return Ok(inherited.unwrap_or_default().into());
    };
    let _lock = store::lock(&path).map_err(|e| e.msg)?;
    let effective = inherited.map(str::to_owned).or(crate::user_data::env_value(home, backend, "CP_SERVER_ID")?).unwrap_or_default();
    let marker = std::fs::read(path.with_extension("env.server-id.json")).ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(marker) = marker {
        let raw = match crate::runtime::queue::read_replaced_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.to_string()),
        };
        let published = marker["published"].as_str();
        let nonce = marker["nonce"].as_str().unwrap_or("");
        if !nonce.is_empty() && published.is_some()
            && inherited == marker["inherited"].as_str()
            && crate::user_data::env_value(home, backend, "CP_SERVER_ID")?.unwrap_or_default() == published.unwrap()
            && raw.lines().any(|line| line.strip_prefix(ID_PUBLICATION) == Some(nonce)) {
            return Ok(published.unwrap().into());
        }
    }
    Ok(effective)
}

#[derive(Debug, Clone)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
    pub msg: String,
}
impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "peers_registro_invalido",
            msg: msg.into(),
        }
    }
    pub fn missing(msg: impl Into<String>) -> Self {
        Self {
            status: 404,
            code: "peers_desconhecido",
            msg: msg.into(),
        }
    }
    pub fn unavailable() -> Self {
        Self {
            status: 503,
            code: "peers_arquivo_ilegivel",
            msg: "não foi possível ler ou gravar a configuração das máquinas".into(),
        }
    }
    pub fn response(self) -> Response {
        json_response(
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail_body(self.code, &self.msg, json!({})),
        )
    }
}
#[derive(Clone)]
pub struct Service {
    pub home: PathBuf,
    pub backend: Option<PathBuf>,
    pub store: store::Store,
    pub env: PathBuf,
    pub token: String,
    pub port: u16,
    pub bind: String,
    pub initial_id: String,
    pub network: Arc<dyn network::Network>,
    pub environment: std::collections::BTreeMap<String, String>,
    settings: Option<Arc<dotenv::SettingsSnapshot>>,
    pub on_identity: Option<Arc<dyn Fn(String) + Send + Sync>>,
}
impl Service {
    pub fn new(
        home: &Path,
        backend: Option<&Path>,
        token: String,
        port: u16,
        bind: String,
        initial_id: String,
    ) -> Result<Self, Error> {
        Self::new_with_environment(home,backend,token,port,bind,initial_id,std::env::vars().collect())
    }
    fn new_with_environment(home:&Path,backend:Option<&Path>,token:String,port:u16,bind:String,initial_id:String,environment:std::collections::BTreeMap<String,String>)->Result<Self,Error>{
        let path = |name| {
            crate::user_data::path(home, backend, name)
                .map_err(|_| Error::unavailable())?
                .ok_or_else(Error::unavailable)
        };
        Ok(Self {
            home: home.into(),
            backend: backend.map(Path::to_path_buf),
            store: store::Store::new(path("peers.json")?),
            env: path(".env")?,
            token,
            port,
            bind,
            initial_id,
            network: Arc::new(network::SystemNetwork),
            environment,
            settings: None,
            on_identity: None,
        })
    }
    pub fn from_state(st: &Arc<AppState>) -> Result<Self, Error> {
        let mut service = Self::new_with_environment(
            &st.accounts.env.home,
            crate::session_open::cli::backend_dir_for_home(&st.accounts.env.home).as_deref(),
            st.cfg.auth_token.clone(),
            st.cfg.listen.port(),
            st.cfg.listen.ip().to_string(),
            st.current_server_id(),
            st.accounts.env.base.clone(),
        )?;
        service.network = st.peer_network.clone();
        service.settings=Some(st.settings_snapshot().map_err(|_|Error::unavailable())?);
        let state = Arc::downgrade(st);
        service.on_identity = Some(Arc::new(move |id| {
            if let Some(state) = state.upgrade() {
                state.set_server_id(id)
            }
        }));
        Ok(service)
    }
    pub fn value(&self, key: &str) -> String {
        if let Some(settings)=&self.settings{return settings.value(key).ok().flatten().unwrap_or_default();}
        config_value_with_environment(&self.home, self.backend.as_deref(), key, &self.environment).ok().flatten().unwrap_or_default()
    }

    pub fn identity(&self) -> String {
        self.initial_id.clone()
    }
    pub fn save_identity(&self, value: &Value) -> Result<String, Error> {
        self.save_identity_using(value, &crate::runtime::queue::atomic_write)
    }
    fn save_identity_using(&self, value: &Value, write: &dyn Fn(&Path, &[u8]) -> std::io::Result<()>) -> Result<String, Error> {
        let value = store::truthy_field(value, "identificador");
        if store::truthy(&value) {
            store::valid_id(&value).map_err(|mut e| {
                e.code = "peers_id_invalido";
                e
            })?;
        }
        let value = value.as_str().unwrap_or("");
        let _lock = store::lock(&self.env)?;
        let old = match crate::runtime::queue::read_replaced_to_string(&self.env) {
            Ok(s) => s.replace("\r\n", "\n").replace('\r', "\n"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return Err(Error::unavailable()),
        };
        let mut seen = false;
        let mut output = String::new();
        for line in old.split_inclusive('\n') {
            if line.starts_with(ID_PUBLICATION) { continue; }
            if line.starts_with("CP_SERVER_ID=") {
                seen = true;
                if !value.is_empty() {
                    output.push_str(&format!("CP_SERVER_ID={value}\n"));
                }
            } else {
                output.push_str(line);
            }
        }
        if !seen && !value.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("CP_SERVER_ID={value}\n"));
        }
        if let Some(inherited) = self.environment.get("CP_SERVER_ID") {
            let mut random = [0u8; 16];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random).map_err(|_| Error::unavailable())?;
            let nonce = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let marker = json!({"inherited": inherited, "published": value, "nonce": nonce});
            write(&self.env.with_extension("env.server-id.json"), &serde_json::to_vec(&marker).map_err(|_| Error::unavailable())?)
                .map_err(|_| Error::unavailable())?;
            if !output.is_empty() && !output.ends_with('\n') { output.push('\n'); }
            output.push_str(&format!("{ID_PUBLICATION}{nonce}\n"));
        }
        store::clean_temporary(&self.env)?;
        write(&self.env, output.as_bytes())
            .map_err(|_| Error::unavailable())?;
        if let Some(changed) = &self.on_identity {
            changed(value.into());
        }
        Ok(value.into())
    }
    pub fn proof(&self, challenge: &str) -> Result<Value, Error> {
        if !(16..=128).contains(&challenge.chars().count()) {
            return Err(Error {
                status: 400,
                code: "peers_desafio_invalido",
                msg: "desafio inválido".into(),
            });
        }
        let id = self.identity();
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.token.as_bytes());
        let digest = ring::hmac::sign(&key, format!("{challenge}|{id}").as_bytes());
        Ok(
            json!({"identificador":id,"prova":digest.as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>()}),
        )
    }
}

/// A credencial ambiente do convidado só aceita origens declaradas; peers desativados continuam válidos.
pub fn origin_allowed(
    home: &Path,
    backend: Option<&Path>,
    origin: &str,
    host: Option<&str>,
    public_url: &str,
    extras: &[String],
) -> bool {
    let authority = |s: &str| {
        let rest = s
            .split_once("://")
            .map(|(_, r)| r)
            .or_else(|| s.strip_prefix("//"))?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        (!authority.is_empty()).then(|| authority.to_owned())
    };
    let Some(target) = authority(origin) else {
        return false;
    };
    if host.is_some_and(|h| h.trim() == target)
        || authority(public_url).as_deref() == Some(target.as_str())
        || extras
            .iter()
            .any(|s| authority(s).unwrap_or_else(|| s.clone()) == target)
    {
        return true;
    }
    let Ok(Some(path)) = crate::user_data::path(home, backend, "peers.json") else {
        return false;
    };
    let data = store::Store::new(path).tolerant();
    let rows: Vec<&Value> = match data.get("peers") {
        Some(Value::Object(entries)) => entries.values().collect(),
        Some(Value::Array(entries)) => entries.iter().collect(),
        Some(_) => Vec::new(),
        None => data.values().collect(),
    };
    rows.into_iter()
        .filter_map(|v| v["base_url"].as_str())
        .any(|s| authority(s).as_deref() == Some(target.as_str()))
}

pub const HTTP_ROUTES:&[(&str,&str)]=&[
    ("/api/peers","GET"),("/api/peers","POST"),
    ("/api/peers/app-list","GET"),("/api/peers/app-list","PUT"),
    ("/api/peers/{id}/enabled","PUT"),("/api/peers/{id}/token","GET"),("/api/peers/{id}","DELETE"),
    ("/api/peers/check","GET"),("/api/peers/ping","GET"),("/api/peers/descobrir","GET"),
    ("/api/peers/identificador","GET"),("/api/peers/prova","GET"),("/api/peers/identificador","PUT"),
    ("/api/peers/anunciar","POST"),("/api/alcance","GET"),("/api/alcance/pareamento","GET"),
];
pub fn owns_http_path(path:&str)->bool{
    ["/api/peers","/api/alcance"].iter().any(|prefix|path==*prefix||path.strip_prefix(prefix).is_some_and(|rest|rest.starts_with('/')))
}
pub async fn handle(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    if let Some(response)=crate::http_cors::preflight(request.method(),request.headers()){return response;}
    let headers=request.headers().clone();
    let method=request.method().clone();
    let mut response=if let Some(response)=crate::http_body_limit::precheck(request.headers()){response}else{handle_inner(State(st),ConnectInfo(peer),request).await};
    crate::http_cors::apply(&headers,&mut response);
    crate::http_route_response::suppress_head(&method,&mut response);
    response
}

async fn handle_inner(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy().into_owned();
    let method = request.method().clone();
    let decision=crate::http_contract::resolve(HTTP_ROUTES,method.as_str(),&path);
    let (_,https)=st.cfg.trusted.resolve(peer.ip(),request.headers());
    if let Some(response)=crate::http_route_response::response(&decision,&method,request.uri(),request.headers(),https,&st.cfg.listen.to_string()){return response;}
    let public =
        method == Method::GET && matches!(path.as_str(), "/api/peers/ping" | "/api/peers/prova");
    if !public && let Err(response) = owner_gate(&st, peer, &request).await {
        return response;
    }
    if path == "/api/peers/ping" && method == Method::GET {
        return json_response(StatusCode::OK, json!({"hangar":true}));
    }
    let query: std::collections::HashMap<String, String> =
        form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let required = match (method.as_str(), path.as_str()) {
        ("GET", "/api/peers/prova") => Some("desafio"),
        ("GET", "/api/alcance/pareamento") => Some("endereco"),
        _ => None,
    };
    if let Some(key) = required && !query.contains_key(key) {
        return json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail":[{"type":"missing","loc":["query",key],"msg":"Field required","input":null}]}));
    }
    let needs_body = (method == Method::POST && path == "/api/peers")
        || (method == Method::PUT
            && (matches!(
                path.as_str(),
                "/api/peers/app-list" | "/api/peers/identificador"
            ) || path.starts_with("/api/peers/") && path.ends_with("/enabled")));
    let body = if needs_body {
        let bytes=match crate::http_body_limit::read(request.into_body()).await {
            Ok(bytes)=>Some(bytes),
            Err(error) if crate::http_body_limit::is_limit(&error)=>return crate::http_body_limit::rejected(),
            Err(_)=>None,
        };
        let value=match bytes{
            Some(bytes)=>match crate::orchestration::body::parse(&bytes){
                Ok(value)=>Some(value),
                Err((StatusCode::BAD_REQUEST,body))=>return json_response(StatusCode::BAD_REQUEST,body),
                Err(_)=>None,
            },
            None=>None,
        };
        match value{
            Some(value) if value.is_object()=>value,
            value=>{
                if let Some(value)=value{crate::orchestration::codec::discard(value);}
                return Error{status:422,code:"erro_corpo_invalido",msg:"corpo inválido".into()}.response();
            }
        }
    } else {
        Value::Null
    };
    let mut body = RequestValue(body);
    let service = match Service::from_state(&st) {
        Ok(s) => s,
        Err(e) => return e.response(),
    };
    let result = dispatch(service, &method, &path, &query, std::mem::take(&mut body.0)).await;
    match result {
        Ok(v) => json_response(StatusCode::OK, v),
        Err(e) if e.code == "alcance_endereco_desconhecido" => json_response(
            StatusCode::NOT_FOUND,
            detail_body(
                e.code,
                &e.msg,
                json!({"endereco":query.get("endereco").cloned().unwrap_or_default()}),
            ),
        ),
        Err(e) => e.response(),
    }
}
struct RequestValue(Value);
impl std::ops::Deref for RequestValue{type Target=Value;fn deref(&self)->&Value{&self.0}}
impl Drop for RequestValue{fn drop(&mut self){crate::orchestration::codec::discard(std::mem::take(&mut self.0));}}
pub async fn dispatch(
    service: Service,
    method: &Method,
    path: &str,
    query: &std::collections::HashMap<String, String>,
    body: Value,
) -> Result<Value, Error> {
    let body=RequestValue(body);
    let q = |key: &str| query.get(key).map(String::as_str).unwrap_or("");
    match (method.as_str(), path) {
        ("GET", "/api/peers/prova") => service.proof(q("desafio")),
        ("GET", "/api/peers/check") => {
            if q("url").is_empty() {
                return Err(Error {
                    status: 400,
                    code: "peers_check_url_obrigatoria",
                    msg: "url obrigatória".into(),
                });
            }
            if q("id").is_empty() {
                return Err(Error {
                    status: 400,
                    code: "peers_check_id_obrigatorio",
                    msg: "id obrigatório".into(),
                });
            }
            Ok(network::check(&service, q("url"), q("id")).await)
        }
        ("GET", "/api/peers/identificador") => {
            Ok(json!({"identificador":service.identity(),"lan_url":network::lan_url(&service)}))
        }
        ("PUT", "/api/peers/identificador") => {
            let old = service.identity();
            let copy = service.clone();
            let id = tokio::task::spawn_blocking(move || copy.save_identity(&body))
                .await
                .map_err(|_| Error::unavailable())??;
            let mut service = service;
            service.initial_id = id.clone();
            let notices = if !id.is_empty() && old != id {
                network::announce(&service, &old).await
            } else {
                json!([])
            };
            Ok(json!({"identificador":id,"avisos":notices}))
        }
        ("POST", "/api/peers/anunciar") => {
            if service.identity().is_empty() {
                return Err(Error {
                    status: 400,
                    code: "peers_sem_identificador",
                    msg: "esta máquina não tem identificador".into(),
                });
            }
            Ok(network::announce(&service, "").await)
        }
        ("GET", "/api/peers/descobrir") => network::discover(&service).await,
        ("GET", "/api/alcance") => Ok(network::reach(&service).await),
        ("GET", "/api/alcance/pareamento") => network::pairing(&service, q("endereco")).await,
        _ => {
            let method = method.clone();
            let path = path.to_owned();
            tokio::task::spawn_blocking(move || match (method.as_str(), path.as_str()) {
                ("GET", "/api/peers") => Ok(service.store.list()),
                ("POST", "/api/peers") => service.store.put(&body),
                ("GET", "/api/peers/app-list") => service.store.app_list(),
                ("PUT", "/api/peers/app-list") => service.store.app_write(&body),
                _ => {
                    let rest = path
                        .strip_prefix("/api/peers/")
                        .ok_or_else(|| Error::missing("rota desconhecida"))?;
                    let decoded = rest;
                    if let Some(id) = decoded
                        .strip_suffix("/enabled")
                        .filter(|_| method == Method::PUT)
                    {
                        service.store.enabled(id, &body["enabled"])
                    } else if let Some(id) = decoded
                        .strip_suffix("/token")
                        .filter(|_| method == Method::GET)
                    {
                        service.store.token(id)
                    } else if method == Method::DELETE {
                        service.store.remove(&decoded)
                    } else {
                        Err(Error {
                            status: 405,
                            code: "peers_metodo_invalido",
                            msg: "método não permitido".into(),
                        })
                    }
                }
            })
            .await
            .map_err(|_| Error::unavailable())?
        }
    }
}

#[cfg(test)]
mod identity_publication_tests {
    use super::*;
    #[test]
    fn publication_failures_never_announce_or_activate_an_uncommitted_identity() {
        for failure in [1, 2] {
            let home=tempfile::tempdir().unwrap();
            let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();
            std::fs::write(config.join("peers.json"),"{}").unwrap();
            let env=config.join(".env");let original="CP_SERVER_ID=after\nOTHER=preserved\n";
            std::fs::write(&env,original).unwrap();
            let mut service=Service::new(home.path(),None,"proof".into(),8765,"127.0.0.1".into(),"before".into()).unwrap();
            service.environment= [("CP_SERVER_ID".into(),"before".into())].into();
            let announced=Arc::new(std::sync::atomic::AtomicBool::new(false));let changed=announced.clone();
            service.on_identity=Some(Arc::new(move |_| {changed.store(true,std::sync::atomic::Ordering::Relaxed);}));
            let calls=std::cell::Cell::new(0);
            let result=service.save_identity_using(&json!({"identificador":"after"}), &|path,bytes| {
                calls.set(calls.get()+1);
                if calls.get()==failure {return Err(std::io::Error::other("falha de publicação simulada"));}
                crate::runtime::queue::atomic_write(path,bytes)
            });
            assert!(result.is_err());assert!(!announced.load(std::sync::atomic::Ordering::Relaxed));
            assert_eq!(std::fs::read_to_string(&env).unwrap(),original);
            assert_eq!(resolve_identity(home.path(),None,Some("before")).unwrap(),"before");
        }
    }
    #[test]
    fn published_identity_only_replaces_the_unchanged_inherited_override() {
        let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("peers.json"),"{}").unwrap();
        std::fs::write(config.join(".env"),"CP_SERVER_ID=before\nOTHER=preserved\n").unwrap();
        let mut service=Service::new(home.path(),None,"proof".into(),8765,"127.0.0.1".into(),"before".into()).unwrap();
        service.environment=[("CP_SERVER_ID".into(),"before".into())].into();
        service.save_identity(&json!({"identificador":"after"})).unwrap();
        assert_eq!(resolve_identity(home.path(),None,Some("before")).unwrap(),"after");
        assert_eq!(resolve_identity(home.path(),None,Some("manual-env")).unwrap(),"manual-env");
        let raw=std::fs::read_to_string(&service.env).unwrap();
        assert_eq!(raw.lines().filter(|line| !line.starts_with(ID_PUBLICATION)).collect::<Vec<_>>(),vec!["CP_SERVER_ID=after","OTHER=preserved"]);
        std::fs::write(&service.env,raw.replace("CP_SERVER_ID=after","CP_SERVER_ID=manual-file")).unwrap();
        assert_eq!(resolve_identity(home.path(),None,Some("before")).unwrap(),"before");
        assert_eq!(resolve_identity(home.path(),None,None).unwrap(),"manual-file");
    }
}
