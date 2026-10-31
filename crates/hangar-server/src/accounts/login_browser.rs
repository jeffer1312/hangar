//! Onde abrir a autorização do login Claude: destinos de cada máquina e validação do link.
use super::catalog::AccountError;
use super::login_view::{Desk, DeskInput, DeskWindow};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// O que o link de autorização diz sobre o retorno: de onde o CLI espera a volta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeLink {
    pub port: u16,
    pub state: String,
}

/// Recusa tudo que não seja a autorização do Claude voltando para um CLI local: este link abre
/// num navegador logado e define a porta local que o Hangar passa a escutar.
pub fn parse_authorize_url(raw: &str) -> Option<AuthorizeLink> {
    let url = reqwest::Url::parse(raw).ok()?;
    let trusted = url.scheme() == "https"
        && matches!(url.host_str(), Some("claude.com" | "claude.ai"))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/cai/oauth/authorize";
    if !trusted {
        return None;
    }
    let param = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value);
    let redirect = param("redirect_uri")?;
    let port = redirect.strip_prefix("http://localhost:")?.strip_suffix("/callback")?;
    if !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    let state = param("state").filter(|state| !state.is_empty())?.into_owned();
    Some(AuthorizeLink { port, state })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromeProfile {
    pub dir: String,
    pub name: String,
    pub email: Option<String>,
}

/// Arquivo ilegível vira lista vazia: a máquina só deixa de oferecer perfis.
pub fn list_chrome_profiles(local_state: &[u8]) -> Vec<ChromeProfile> {
    let Ok(raw) = serde_json::from_slice::<serde_json::Value>(local_state) else {
        return Vec::new();
    };
    let Some(cache) = raw["profile"]["info_cache"].as_object() else {
        return Vec::new();
    };
    let text = |value: &serde_json::Value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned);
    let mut profiles: Vec<ChromeProfile> = cache
        .iter()
        .map(|(dir, info)| ChromeProfile {
            dir: dir.clone(),
            name: text(&info["name"]).unwrap_or_else(|| dir.clone()),
            email: text(&info["user_name"]),
        })
        .collect();
    profiles.sort_by(|a, b| a.dir.cmp(&b.dir));
    profiles
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    ChromeProfile,
    DefaultBrowser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub machine: String,
    pub kind: TargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Windows,
    Other,
}

/// O que a máquina tem instalado; separado da detecção para testar Windows também no Linux.
#[derive(Debug, Clone)]
pub struct BrowserHost {
    pub platform: Platform,
    pub chrome: Option<PathBuf>,
    pub local_state: Option<Vec<u8>>,
    pub default_opener: Option<PathBuf>,
}

pub fn local_targets(machine: &str, host: &BrowserHost) -> Vec<Target> {
    if host.platform == Platform::Other {
        return Vec::new();
    }
    let mut targets = Vec::new();
    if let (Some(_), Some(local_state)) = (&host.chrome, &host.local_state) {
        targets.extend(list_chrome_profiles(local_state).into_iter().map(|profile| Target {
            machine: machine.to_owned(),
            kind: TargetKind::ChromeProfile,
            profile: Some(profile.dir),
            label: profile.name,
            email: profile.email,
        }));
    }
    if host.default_opener.is_some() {
        targets.push(Target {
            machine: machine.to_owned(),
            kind: TargetKind::DefaultBrowser,
            profile: None,
            label: String::new(),
            email: None,
        });
    }
    targets
}

pub(crate) fn current_platform() -> Platform {
    if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Other
    }
}

fn find_on_path(name: &str, path: &str) -> Option<PathBuf> {
    std::env::split_paths(path).map(|dir| dir.join(name)).find(|file| file.is_file())
}

/// Bloqueia (lê arquivos): quem chama faz fora da thread do runtime.
pub fn detect_host() -> BrowserHost {
    detect_host_from(current_platform(), |key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

fn detect_host_from(platform: Platform, env: impl Fn(&str) -> Option<String>) -> BrowserHost {
    let (chrome, state_file, default_opener) = match platform {
        Platform::Linux => {
            let path = env("PATH").unwrap_or_default();
            (
                find_on_path("google-chrome-stable", &path),
                env("HOME").map(|home| PathBuf::from(home).join(".config/google-chrome/Local State")),
                find_on_path("xdg-open", &path),
            )
        }
        Platform::Windows => {
            // Só as instalações padrão, com a de usuário primeiro: é a que o usuário atual abre.
            let local = env("LOCALAPPDATA").map(PathBuf::from);
            let chrome = local
                .iter()
                .cloned()
                .chain(env("ProgramFiles").map(PathBuf::from))
                .chain(env("ProgramFiles(x86)").map(PathBuf::from))
                .map(|root| root.join("Google").join("Chrome").join("Application").join("chrome.exe"))
                .find(|file| file.is_file());
            (
                chrome,
                local.map(|root| root.join("Google").join("Chrome").join("User Data").join("Local State")),
                env("SystemRoot")
                    .map(|root| PathBuf::from(root).join("System32").join("rundll32.exe"))
                    .filter(|file| file.is_file()),
            )
        }
        Platform::Other => (None, None, None),
    };
    BrowserHost {
        platform,
        chrome,
        local_state: state_file.and_then(|file| std::fs::read(file).ok()),
        default_opener,
    }
}

/// Uma abertura: programa e argumentos, sem shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// `None` quando a máquina não oferece mais o destino: o perfil vem de outra máquina e só vale
/// se existir aqui, o que também impede um valor qualquer de virar opção do Chrome.
pub fn browser_command(host: &BrowserHost, target: &Target, url: &str) -> Option<BrowserCommand> {
    match (host.platform, target.kind) {
        (Platform::Other, _) => None,
        (_, TargetKind::ChromeProfile) => {
            let dir = target.profile.as_deref()?;
            list_chrome_profiles(host.local_state.as_deref()?).iter().find(|profile| profile.dir == dir)?;
            Some(BrowserCommand {
                program: host.chrome.clone()?,
                // Janela própria: é ela que a transmissão acompanha e que o fim do login fecha.
                args: vec![format!("--profile-directory={dir}"), "--new-window".to_owned(), url.to_owned()],
            })
        }
        (Platform::Linux, TargetKind::DefaultBrowser) => Some(BrowserCommand {
            program: host.default_opener.clone()?,
            args: vec![url.to_owned()],
        }),
        // `cmd /c start` cortaria o link no primeiro `&`.
        (Platform::Windows, TargetKind::DefaultBrowser) => Some(BrowserCommand {
            program: host.default_opener.clone()?,
            args: vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()],
        }),
    }
}

/// `machine` passa a ser o id do `peers.json`, que é como esta máquina chama a outra.
pub fn merge_targets(
    local: Vec<Target>,
    peers: Vec<(String, Option<Vec<Target>>)>,
    last: Option<Target>,
) -> (Vec<Target>, Option<Target>) {
    let mut targets = local;
    for (id, offered) in peers {
        targets.extend(offered.into_iter().flatten().map(|target| Target { machine: id.clone(), ..target }));
    }
    let last = last.and_then(|last| {
        targets
            .iter()
            .find(|target| target.machine == last.machine && target.kind == last.kind && target.profile == last.profile)
            .cloned()
    });
    (targets, last)
}

pub fn url_file_name(operation: &str) -> String {
    format!("claude-login-url-{operation}")
}

/// Nome e conteúdo do programa que o CLI chama como `BROWSER`: só guarda o link ao lado dele, e o
/// Rust decide onde abrir. `url` é o nome final do arquivo do link, na mesma pasta do script. `None`
/// onde o login segue pelo link impresso.
pub fn browser_script(platform: Platform, operation: &str, url: &str) -> Option<(String, String)> {
    match platform {
        Platform::Linux => Some((
            format!("claude-login-browser-{operation}"),
            format!("#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$1\" > \"$d/{url}.tmp\" && mv \"$d/{url}.tmp\" \"$d/{url}\"\n"),
        )),
        Platform::Windows => Some((
            format!("claude-login-browser-{operation}.cmd"),
            format!("@echo off\r\n>\"%~dp0{url}\" echo %*\r\n"),
        )),
        Platform::Other => None,
    }
}

/// Tudo que a tentativa pode deixar na pasta da conta.
pub fn attempt_file_names(operation: &str) -> [String; 4] {
    let url = url_file_name(operation);
    [
        format!("claude-login-browser-{operation}"),
        format!("claude-login-browser-{operation}.cmd"),
        format!("{url}.tmp"),
        url,
    ]
}

/// No Windows o link chega entre aspas, e o `echo` deixa espaço e quebra de linha no fim.
pub fn clean_url(raw: &str) -> &str {
    raw.trim().trim_matches('"').trim()
}

pub fn write_browser_script(file: &std::path::Path, content: &str) -> std::io::Result<()> {
    std::fs::write(file, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Corpo do `POST /login`: vazio continua valendo e não abre navegador.
pub fn target_from_body(body: &[u8]) -> Result<Option<Target>, ()> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    #[derive(Deserialize)]
    struct LoginBody {
        #[serde(default)]
        target: Option<Target>,
    }
    serde_json::from_slice::<LoginBody>(body).map(|parsed| parsed.target).map_err(|_| ())
}

/// Como a tentativa fala com a máquina do navegador; os testes trocam por um falso.
pub trait BrowserChannel: Send + Sync {
    fn is_local(&self, machine: &str) -> bool;
    /// Erro = motivo curto, para a tela: o código devolvido pela máquina ou a falta de resposta.
    fn open(&self, request: OpenRequest) -> futures_util::future::BoxFuture<'_, Result<(), String>>;
    /// Erro = não deu para perguntar agora, e a consulta tenta de novo.
    fn result<'a>(&'a self, machine: &'a str, operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<Option<String>, ()>>;
    /// A janela do login, pela máquina do navegador.
    fn view<'a>(&'a self, _machine: &'a str, _operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<serde_json::Value, ()>> {
        Box::pin(async { Err(()) })
    }
    fn input<'a>(&'a self, _machine: &'a str, _operation: &'a str, _input: serde_json::Value) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("erro_login_machine_unreachable".to_owned()) })
    }
    fn activate<'a>(&'a self, _machine: &'a str, _operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<bool, ()>> {
        Box::pin(async { Err(()) })
    }
    fn close<'a>(&'a self, _machine: &'a str, _operation: &'a str) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

pub struct AppChannel {
    pub peers: Arc<crate::groups::peers::PeerClient>,
    pub server_id: String,
    pub browser: Arc<LoginBrowser>,
}

impl BrowserChannel for AppChannel {
    fn is_local(&self, machine: &str) -> bool {
        machine == self.server_id
    }

    fn open(&self, request: OpenRequest) -> futures_util::future::BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.is_local(&request.target.machine) {
                return self.browser.open(request).await.map_err(|error| error.code.to_string());
            }
            let machine = request.target.machine.clone();
            let body = serde_json::to_value(&request).map_err(|_| "login_request_invalid".to_string())?;
            match self.peers.call(&machine, reqwest::Method::POST, "/api/login-browser/open", Some(&body)).await {
                Ok(_) => Ok(()),
                Err(crate::groups::peers::PeerError::Refused { detail, .. }) => {
                    Err(detail["code"].as_str().unwrap_or("erro_login_browser_failed").to_owned())
                }
                Err(_) => Err("erro_login_machine_unreachable".to_owned()),
            }
        })
    }

    fn result<'a>(&'a self, machine: &'a str, operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<Option<String>, ()>> {
        Box::pin(async move {
            if self.is_local(machine) {
                return Ok(self.browser.result(operation));
            }
            let path = format!("/api/login-browser/result?operation={operation}");
            match self.peers.call(machine, reqwest::Method::GET, &path, None).await {
                Ok(Some(reply)) => Ok(reply["query"].as_str().map(str::to_owned)),
                _ => Err(()),
            }
        })
    }

    fn view<'a>(&'a self, machine: &'a str, operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<serde_json::Value, ()>> {
        Box::pin(async move {
            if self.is_local(machine) {
                return Ok(self.browser.view(operation).await);
            }
            let path = format!("/api/login-browser/view?operation={operation}");
            match self.peers.call(machine, reqwest::Method::GET, &path, None).await {
                Ok(Some(reply)) => Ok(reply),
                _ => Err(()),
            }
        })
    }

    fn input<'a>(&'a self, machine: &'a str, operation: &'a str, input: serde_json::Value) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if self.is_local(machine) {
                let input = serde_json::from_value(input).map_err(|_| "erro_login_view_input_invalid".to_owned())?;
                return self.browser.input(operation, input).await.map_err(|error| error.code.to_string());
            }
            let path = format!("/api/login-browser/input?operation={operation}");
            match self.peers.call(machine, reqwest::Method::POST, &path, Some(&input)).await {
                Ok(_) => Ok(()),
                Err(crate::groups::peers::PeerError::Refused { detail, .. }) => {
                    Err(detail["code"].as_str().unwrap_or("erro_login_view_input_invalid").to_owned())
                }
                Err(_) => Err("erro_login_machine_unreachable".to_owned()),
            }
        })
    }

    fn activate<'a>(&'a self, machine: &'a str, operation: &'a str) -> futures_util::future::BoxFuture<'a, Result<bool, ()>> {
        Box::pin(async move {
            if self.is_local(machine) {
                return Ok(self.browser.activate(operation).await);
            }
            let path = format!("/api/login-browser/activate?operation={operation}");
            match self.peers.call(machine, reqwest::Method::POST, &path, Some(&serde_json::json!({}))).await {
                Ok(Some(reply)) => Ok(reply["ok"] == true),
                _ => Err(()),
            }
        })
    }

    fn close<'a>(&'a self, machine: &'a str, operation: &'a str) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            if self.is_local(machine) {
                self.browser.close_view(operation).await;
                return;
            }
            let path = format!("/api/login-browser/close?operation={operation}");
            let _ = self.peers.call(machine, reqwest::Method::POST, &path, Some(&serde_json::json!({}))).await;
        })
    }
}

const LISTEN_CAP: std::time::Duration = std::time::Duration::from_secs(600);
const CLOSE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Hangar</title>\
    <p>Pode fechar esta aba. / You can close this tab.</p>";

pub type Launcher = Arc<dyn Fn(BrowserCommand) -> std::io::Result<()> + Send + Sync>;
pub type HostProbe = Arc<dyn Fn() -> BrowserHost + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRequest {
    pub url: String,
    pub operation: String,
    pub target: Target,
    #[serde(default)]
    pub remote: bool,
}

/// Abertura do navegador nesta máquina e, quando a conta está em outra, a escuta do retorno.
pub struct LoginBrowser {
    listens: Listens,
    launcher: Launcher,
    probe: HostProbe,
    generation: AtomicU64,
    views: Views,
    desk_probe: DeskProbe,
}

pub type DeskProbe = Arc<dyn Fn() -> Option<Arc<dyn Desk>> + Send + Sync>;

/// Título da página de autorização, do login dela ou da volta para o CLI. Janela nova com outro título é
/// da pessoa: o Chrome frio restaura as dela junto, e a do login nunca pode ser confundida com elas.
fn is_login_window(window: &DeskWindow) -> bool {
    let title = window.title.to_lowercase();
    ["claude", "anthropic", "hangar"].iter().any(|name| title.contains(name))
}

/// A janela que o login abriu: a primeira nova com título de login, e nenhuma outra.
struct View {
    desk: Arc<dyn Desk>,
    baseline: HashSet<String>,
    window: Option<String>,
}

type Views = Arc<Mutex<HashMap<String, View>>>;

fn locked_views(views: &Views) -> std::sync::MutexGuard<'_, HashMap<String, View>> {
    views.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Fecha só a janela do login; o resto do Chrome da pessoa continua aberto.
async fn close_view_in(views: &Views, operation: &str) {
    let Some(view) = locked_views(views).remove(operation) else { return };
    let _ = tokio::task::spawn_blocking(move || {
        let windows = view.desk.windows().into_iter().flatten();
        // Quem não abriu a transmissão ainda tem a janela aberta. O título confere de novo: o endereço de
        // uma janela fechada pode já ser de outra.
        let found = match &view.window {
            Some(id) => windows.into_iter().find(|window| &window.id == id),
            None => windows.into_iter().find(|window| !view.baseline.contains(&window.id) && is_login_window(window)),
        };
        if let Some(window) = found.filter(is_login_window) {
            view.desk.close(&window);
        }
    })
    .await;
}

const VIEW_CAP: std::time::Duration = std::time::Duration::from_secs(5);
/// Deixa a pessoa ver a confirmação antes de a janela sumir.
const CLOSE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

struct Listen {
    generation: u64,
    query: Option<String>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

type Listens = Arc<Mutex<HashMap<String, Listen>>>;

fn locked(listens: &Listens) -> std::sync::MutexGuard<'_, HashMap<String, Listen>> {
    listens.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
struct CallbackState {
    listens: Listens,
    operation: String,
    generation: u64,
    state: String,
    views: Views,
}

/// Pedido com `state` errado não é guardado nem fecha a escuta: entregue ao CLI, derrubaria o login.
async fn callback(
    axum::extract::State(expected): axum::extract::State<CallbackState>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let query = query.unwrap_or_default();
    let matches = form_urlencoded::parse(query.as_bytes())
        .any(|(key, value)| key == "state" && value == expected.state.as_str());
    let accepted = matches && {
        let mut listens = locked(&expected.listens);
        match listens
            .get_mut(&expected.operation)
            .filter(|listen| listen.generation == expected.generation && listen.query.is_none())
        {
            Some(listen) => {
                listen.query = Some(query);
                if let Some(stop) = listen.stop.take() {
                    let _ = stop.send(());
                }
                true
            }
            None => false,
        }
    };
    if !accepted {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let (views, operation) = (expected.views.clone(), expected.operation.clone());
    tokio::spawn(async move {
        tokio::time::sleep(CLOSE_DELAY).await;
        close_view_in(&views, &operation).await;
    });
    ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], CLOSE_PAGE).into_response()
}

fn refused(status: u16, code: &'static str, message: &str) -> AccountError {
    AccountError::new(status, code, message, serde_json::Value::Null)
}

impl LoginBrowser {
    pub fn new() -> Self {
        Self::with_desk(Arc::new(launch_detached), Arc::new(detect_host), Arc::new(super::login_view::detect))
    }

    pub fn with(launcher: Launcher, probe: HostProbe) -> Self {
        Self::with_desk(launcher, probe, Arc::new(|| None))
    }

    pub fn with_desk(launcher: Launcher, probe: HostProbe, desk_probe: DeskProbe) -> Self {
        Self { listens: Arc::default(), launcher, probe, generation: AtomicU64::new(0), views: Arc::default(), desk_probe }
    }

    /// Antes de lançar: o que já está na tela nunca é transmitido nem fechado.
    async fn prepare_view(&self, operation: &str) {
        let probe = self.desk_probe.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let desk = probe()?;
            // Sem saber o que já estava na tela, qualquer janela poderia virar a do login.
            let baseline = desk.windows()?.into_iter().map(|window| window.id).collect();
            Some(View { desk, baseline, window: None })
        })
        .await;
        if let Ok(Some(view)) = prepared {
            locked_views(&self.views).insert(operation.to_owned(), view);
        }
    }

    /// Um quadro da janela do login, em JSON: é o que o canal entre máquinas leva.
    pub async fn view(&self, operation: &str) -> serde_json::Value {
        use base64::Engine;
        use serde_json::json;
        let Some((desk, baseline, known)) = locked_views(&self.views)
            .get(operation)
            .map(|view| (view.desk.clone(), view.baseline.clone(), view.window.clone()))
        else {
            return json!({"available": false});
        };
        let looked = known.is_some();
        let found = tokio::time::timeout(VIEW_CAP, tokio::task::spawn_blocking(move || {
            let Some(windows) = desk.windows() else { return None };
            let (window, first) = match &known {
                Some(id) => (windows.into_iter().find(|window| &window.id == id), false),
                None => (windows.into_iter().find(|window| !baseline.contains(&window.id) && is_login_window(window)), true),
            };
            let Some(mut window) = window else { return Some((None, None, true)) };
            if first {
                desk.present(&window);
                if let Some(moved) = desk.windows().into_iter().flatten().find(|other| other.id == window.id) {
                    window = moved;
                }
            }
            let jpeg = desk.capture(&window);
            let can_input = desk.can_input();
            Some((Some(window), jpeg, can_input))
        }))
        .await;
        let Ok(Ok(Some((window, jpeg, can_input)))) = found else {
            return json!({"available": true, "retry": true});
        };
        let Some(window) = window else {
            return if looked { json!({"available": true, "closed": true}) } else { json!({"available": true, "pending": true}) };
        };
        if let Some(view) = locked_views(&self.views).get_mut(operation) {
            view.window = Some(window.id.clone());
        }
        match jpeg {
            Some(jpeg) => json!({
                "available": true,
                "d": base64::engine::general_purpose::STANDARD.encode(jpeg),
                "w": window.w,
                "h": window.h,
                "title": window.title,
                "input": can_input,
            }),
            None => json!({"available": true, "error": "erro_login_view_capture", "input": can_input}),
        }
    }

    pub async fn input(&self, operation: &str, input: DeskInput) -> Result<(), AccountError> {
        if matches!(&input, DeskInput::Text { text } if text.chars().count() > 4096) {
            return Err(refused(422, "erro_login_view_input_invalid", "texto grande demais"));
        }
        let view = locked_views(&self.views).get(operation).and_then(|view| Some((view.desk.clone(), view.window.clone()?)));
        let Some((desk, id)) = view else {
            return Err(refused(409, "erro_login_view_missing", "a janela do login não está aberta"));
        };
        let delivered = tokio::time::timeout(VIEW_CAP, tokio::task::spawn_blocking(move || {
            if !desk.can_input() {
                return Err(refused(409, "erro_login_view_no_input", "a sessão desta máquina está desconectada"));
            }
            let window = desk.windows().into_iter().flatten().find(|window| window.id == id)
                .ok_or_else(|| refused(409, "erro_login_view_missing", "a janela do login não está aberta"))?;
            if desk.input(&window, &input) { Ok(()) } else {
                Err(refused(409, "erro_login_view_input_failed", "a entrada não chegou à janela do login"))
            }
        }))
        .await;
        match delivered {
            Ok(Ok(result)) => result,
            _ => Err(refused(504, "erro_login_view_input_timeout", "a máquina não respondeu")),
        }
    }

    pub async fn activate(&self, operation: &str) -> bool {
        let Some(desk) = locked_views(&self.views).get(operation).map(|view| view.desk.clone()) else { return false };
        // Sessão ativa não se reativa: tomaria a conexão de quem está nela.
        matches!(tokio::time::timeout(VIEW_CAP * 3, tokio::task::spawn_blocking(move || desk.can_input() || desk.activate())).await, Ok(Ok(true)))
    }

    pub async fn close_view(&self, operation: &str) {
        close_view_in(&self.views, operation).await;
    }

    /// Bloqueia só em `spawn_blocking`: a detecção lê arquivos e o lançamento espera o processo.
    pub async fn targets(&self, machine: &str) -> Vec<Target> {
        let probe = self.probe.clone();
        let host = tokio::task::spawn_blocking(move || probe()).await;
        host.map(|host| local_targets(machine, &host)).unwrap_or_default()
    }

    pub async fn open(&self, request: OpenRequest) -> Result<(), AccountError> {
        let link = parse_authorize_url(&request.url)
            .ok_or_else(|| refused(422, "erro_login_link_refused", "link de autorização recusado"))?;
        let probe = self.probe.clone();
        let host = tokio::task::spawn_blocking(move || probe()).await.map_err(|_| AccountError::io())?;
        let command = browser_command(&host, &request.target, &request.url)
            .ok_or_else(|| refused(409, "erro_login_target_missing", "esse navegador não está mais disponível nesta máquina"))?;
        self.stop(&request.operation);
        if request.remote {
            self.listen(&request.operation, &link).await?;
        }
        if request.target.kind == TargetKind::ChromeProfile {
            self.prepare_view(&request.operation).await;
        }
        let launcher = self.launcher.clone();
        let launched = tokio::task::spawn_blocking(move || launcher(command)).await;
        if !matches!(launched, Ok(Ok(()))) {
            locked_views(&self.views).remove(&request.operation);
            self.stop(&request.operation);
            return Err(refused(502, "erro_login_browser_failed", "não consegui abrir o navegador"));
        }
        Ok(())
    }

    /// A query sai uma vez só: quem a recebe é o único que pode entregá-la ao CLI.
    pub fn result(&self, operation: &str) -> Option<String> {
        let mut listens = locked(&self.listens);
        listens.get(operation)?.query.as_ref()?;
        listens.remove(operation)?.query
    }

    fn stop(&self, operation: &str) {
        if let Some(listen) = locked(&self.listens).remove(operation)
            && let Some(stop) = listen.stop
        {
            let _ = stop.send(());
        }
    }

    async fn listen(&self, operation: &str, link: &AuthorizeLink) -> Result<(), AccountError> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, link.port))
            .await
            .map_err(|_| refused(409, "erro_login_port_busy", "a porta do retorno do login está ocupada nesta máquina"))?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        locked(&self.listens)
            .insert(operation.to_owned(), Listen { generation, query: None, stop: Some(stop) });
        let router = axum::Router::new()
            .route("/callback", axum::routing::get(callback))
            .with_state(CallbackState {
                listens: self.listens.clone(),
                operation: operation.to_owned(),
                generation,
                state: link.state.clone(),
                views: self.views.clone(),
            });
        let serve = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        let (listens, views, operation) = (self.listens.clone(), self.views.clone(), operation.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(LISTEN_CAP).await;
            // O navegador pode manter conexão parada, e a espera graciosa não terminaria sozinha.
            serve.abort();
            let expired = {
                let mut listens = locked(&listens);
                let current = listens.get(&operation).is_some_and(|listen| listen.generation == generation);
                if current {
                    listens.remove(&operation);
                }
                current
            };
            if expired {
                close_view_in(&views, &operation).await;
            }
        });
        Ok(())
    }
}

impl Default for LoginBrowser {
    fn default() -> Self {
        Self::new()
    }
}

/// O navegador fica fora do serviço para não morrer quando o Hangar reiniciar. Quem já estava
/// aberto recebe o link e o processo sai na hora; quem acabou de nascer fica, e não tem como
/// falhar depois do prazo curto que só separa as duas situações de um erro imediato.
fn launch_detached(command: BrowserCommand) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    #[cfg(unix)]
    let mut process = {
        use std::os::unix::process::CommandExt;
        let prefix = crate::runtime::process::scope_prefix();
        let mut process = match prefix.split_first() {
            Some((first, rest)) => {
                let mut process = Command::new(first);
                process.args(rest).arg(&command.program);
                process
            }
            None => Command::new(&command.program),
        };
        // SAFETY: entre o fork e o exec só roda `setsid`, que é async-signal-safe e não aloca.
        unsafe {
            process.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        process
    };
    #[cfg(not(unix))]
    let mut process = Command::new(&command.program);
    let display = crate::display_env::for_launch(|key| std::env::var(key).ok());
    let mut child = process
        .args(&command.args)
        .envs(display)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    for _ in 0..15 {
        if let Some(status) = child.try_wait()? {
            return if status.success() { Ok(()) } else { Err(std::io::Error::other(status.to_string())) };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::spawn(move || child.wait());
    Ok(())
}

impl super::AccountService {
    fn login_target_file(&self, label: &str) -> Option<PathBuf> {
        let account = self.claude_by_label(label).ok()?;
        let key = super::AccountKey::new(super::Provider::Claude, &account.home).ok()?;
        self.sidecar(&key, "claude-login-target.json").ok()
    }

    pub fn last_login_target(&self, label: &str) -> Option<Target> {
        let raw = std::fs::read(self.login_target_file(label)?).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Sem nome nem email: esses vêm da leitura atual da máquina, não da época do login.
    pub fn remember_login_target(&self, label: &str, target: &Target) -> std::io::Result<()> {
        let file = self.login_target_file(label).ok_or_else(|| std::io::Error::other("conta desconhecida"))?;
        let stored = Target { label: String::new(), email: None, ..target.clone() };
        crate::runtime::queue::atomic_write(&file, serde_json::to_string(&stored)?.as_bytes())
    }
}

const LOGIN_TARGETS_PREFIX: &str = "/api/conta-estado/";
const LOGIN_TARGETS_SUFFIX: &str = "/login/targets";
const PEER_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

fn account_of_targets_path(path: &str) -> Option<&str> {
    path.strip_prefix(LOGIN_TARGETS_PREFIX)?
        .strip_suffix(LOGIN_TARGETS_SUFFIX)
        .filter(|label| !label.is_empty() && !label.contains('/'))
}

/// `/api/conta-estado/<conta>/login/view[/input|/activate]`: o cliente só fala com a máquina da conta.
fn account_view_path(path: &str) -> Option<(&str, &str)> {
    let (label, action) = path.strip_prefix(LOGIN_TARGETS_PREFIX)?.split_once("/login/view")?;
    (!label.is_empty() && !label.contains('/') && matches!(action, "" | "/input" | "/activate")).then_some((label, action))
}

pub(crate) fn matches(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    match path {
        "/api/login-browser/targets" | "/api/login-browser/result" | "/api/login-browser/view" => *method == Method::GET,
        "/api/login-browser/open" | "/api/login-browser/input" | "/api/login-browser/close" | "/api/login-browser/activate" => {
            *method == Method::POST
        }
        _ => {
            if let Some((_, action)) = account_view_path(path) {
                return if action.is_empty() { *method == Method::GET } else { *method == Method::POST };
            }
            *method == Method::GET && account_of_targets_path(path).is_some()
        }
    }
}

fn operation_of(uri: &axum::http::Uri) -> String {
    form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .find(|(key, _)| key == "operation")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

async fn account_view(state: Arc<crate::routes::AppState>, request: axum::extract::Request, label: &str, action: &str) -> axum::response::Response {
    use super::http::Json;
    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::json;
    let Ok(label) = percent_encoding::percent_decode_str(label).decode_utf8() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some((machine, operation, channel)) = state.accounts.login_view_route(&label).await else {
        return Json(json!({"available": false})).into_response();
    };
    match action {
        "" => match channel.view(&machine, &operation).await {
            Ok(frame) => Json(frame).into_response(),
            Err(()) => Json(json!({"available": true, "error": "erro_login_machine_unreachable"})).into_response(),
        },
        "/input" => {
            let Ok(bytes) = axum::body::to_bytes(request.into_body(), 16384).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            let Ok(input) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                return StatusCode::UNPROCESSABLE_ENTITY.into_response();
            };
            match channel.input(&machine, &operation, input).await {
                Ok(()) => Json(json!({"ok": true})).into_response(),
                Err(code) => (StatusCode::CONFLICT, Json(json!({"code": code}))).into_response(),
            }
        }
        _ => match channel.activate(&machine, &operation).await {
            Ok(ok) => Json(json!({"ok": ok})).into_response(),
            Err(()) => (StatusCode::BAD_GATEWAY, Json(json!({"code": "erro_login_machine_unreachable"}))).into_response(),
        },
    }
}

async fn peer_targets(state: &crate::routes::AppState) -> Vec<(String, Option<Vec<Target>>)> {
    let mut ids = state.peers.enabled_ids();
    ids.retain(|id| *id != state.server_id);
    ids.sort();
    let mut asking = tokio::task::JoinSet::new();
    for (index, id) in ids.iter().cloned().enumerate() {
        let peers = state.peers.clone();
        asking.spawn(async move {
            let call = peers.call(&id, reqwest::Method::GET, "/api/login-browser/targets", None);
            let reply = tokio::time::timeout(PEER_WAIT, call).await;
            let offered = match reply {
                Ok(Ok(Some(value))) => serde_json::from_value(value["targets"].clone()).ok(),
                _ => None,
            };
            (index, id, offered)
        });
    }
    let mut answers: Vec<(usize, String, Option<Vec<Target>>)> = asking.join_all().await;
    answers.sort_by_key(|(index, ..)| *index);
    answers.into_iter().map(|(_, id, offered)| (id, offered)).collect()
}

pub(crate) async fn public(state: Arc<crate::routes::AppState>, request: axum::extract::Request) -> axum::response::Response {
    use super::http::Json;
    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::json;
    let path = request.uri().path().to_owned();
    match path.as_str() {
        "/api/login-browser/targets" => {
            let targets = state.login_browser.targets(&state.server_id).await;
            Json(json!({"machine": state.server_id, "targets": targets})).into_response()
        }
        "/api/login-browser/result" => {
            Json(json!({"query": state.login_browser.result(&operation_of(request.uri()))})).into_response()
        }
        "/api/login-browser/view" => Json(state.login_browser.view(&operation_of(request.uri())).await).into_response(),
        "/api/login-browser/input" => {
            let operation = operation_of(request.uri());
            let Ok(bytes) = axum::body::to_bytes(request.into_body(), 16384).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            let Ok(input) = serde_json::from_slice::<DeskInput>(&bytes) else {
                return StatusCode::UNPROCESSABLE_ENTITY.into_response();
            };
            match state.login_browser.input(&operation, input).await {
                Ok(()) => Json(json!({"ok": true})).into_response(),
                Err(error) => super::http::error(error),
            }
        }
        "/api/login-browser/close" => {
            state.login_browser.close_view(&operation_of(request.uri())).await;
            Json(json!({"ok": true})).into_response()
        }
        "/api/login-browser/activate" => {
            Json(json!({"ok": state.login_browser.activate(&operation_of(request.uri())).await})).into_response()
        }
        "/api/login-browser/open" => {
            let Ok(bytes) = axum::body::to_bytes(request.into_body(), 16384).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            let Ok(open) = serde_json::from_slice::<OpenRequest>(&bytes) else {
                return StatusCode::UNPROCESSABLE_ENTITY.into_response();
            };
            match state.login_browser.open(open).await {
                Ok(()) => Json(json!({"ok": true})).into_response(),
                Err(error) => super::http::error(error),
            }
        }
        _ => {
            if let Some((label, action)) = account_view_path(&path) {
                return account_view(state, request, label, action).await;
            }
            let Some(label) = account_of_targets_path(&path) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let Ok(label) = percent_encoding::percent_decode_str(label).decode_utf8() else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let local = state.login_browser.targets(&state.server_id).await;
            let last = state.accounts.last_login_target(&label);
            let peers = peer_targets(&state).await;
            // Quem não respondeu continua na lista: a pessoa precisa ver a máquina e saber por que não dá.
            let machines: Vec<_> = std::iter::once(json!({"id": state.server_id, "answered": true}))
                .chain(peers.iter().map(|(id, offered)| json!({"id": id, "answered": offered.is_some()})))
                .collect();
            let (targets, last) = merge_targets(local, peers, last);
            Json(json!({"targets": targets, "last": last, "machines": machines})).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=abc\
        &response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A34047%2Fcallback\
        &scope=user%3Aprofile&code_challenge=xyz&code_challenge_method=S256&state=st4te";

    fn with_redirect(redirect: &str) -> String {
        format!("https://claude.com/cai/oauth/authorize?redirect_uri={redirect}&state=s")
    }

    /// Tela falsa: as janelas mudam pelo teste, e cada ação fica registrada.
    #[derive(Default)]
    struct FakeDesk {
        windows: Mutex<Vec<crate::accounts::login_view::DeskWindow>>,
        log: Mutex<Vec<String>>,
        no_input: std::sync::atomic::AtomicBool,
        unreadable: std::sync::atomic::AtomicBool,
        input_fails: std::sync::atomic::AtomicBool,
    }
    impl Desk for FakeDesk {
        fn windows(&self) -> Option<Vec<crate::accounts::login_view::DeskWindow>> {
            (!self.unreadable.load(Ordering::Relaxed)).then(|| self.windows.lock().unwrap().clone())
        }
        fn present(&self, window: &crate::accounts::login_view::DeskWindow) {
            self.log.lock().unwrap().push(format!("present {}", window.id));
        }
        fn capture(&self, window: &crate::accounts::login_view::DeskWindow) -> Option<Vec<u8>> {
            Some(window.id.clone().into_bytes())
        }
        fn can_input(&self) -> bool {
            !self.no_input.load(Ordering::Relaxed)
        }
        fn input(&self, window: &crate::accounts::login_view::DeskWindow, input: &DeskInput) -> bool {
            self.log.lock().unwrap().push(format!("input {} {input:?}", window.id));
            !self.input_fails.load(Ordering::Relaxed)
        }
        fn activate(&self) -> bool {
            self.log.lock().unwrap().push("activate".to_owned());
            true
        }
        fn close(&self, window: &crate::accounts::login_view::DeskWindow) {
            self.log.lock().unwrap().push(format!("close {}", window.id));
        }
    }
    fn desk_window(id: &str) -> crate::accounts::login_view::DeskWindow {
        crate::accounts::login_view::DeskWindow { id: id.into(), title: format!("Claude {id}"), x: 0, y: 0, w: 100, h: 50, capture_id: None }
    }
    fn someone_elses_window(id: &str) -> crate::accounts::login_view::DeskWindow {
        crate::accounts::login_view::DeskWindow { title: "Nova guia - Google Chrome".into(), ..desk_window(id) }
    }
    fn viewing(desk: Arc<FakeDesk>) -> LoginBrowser {
        let probe = desk.clone();
        LoginBrowser::with_desk(
            Arc::new(|_| Ok(())),
            Arc::new(|| host(Platform::Linux, true, true)),
            Arc::new(move || Some(probe.clone() as Arc<dyn Desk>)),
        )
    }
    fn view_request(kind: TargetKind) -> OpenRequest {
        OpenRequest {
            url: LINK.to_owned(),
            operation: "op".to_owned(),
            target: Target {
                machine: "m".into(),
                kind,
                profile: (kind == TargetKind::ChromeProfile).then(|| "Default".to_owned()),
                label: String::new(),
                email: None,
            },
            remote: false,
        }
    }

    #[tokio::test]
    async fn view_follows_only_the_window_the_login_opened() {
        let desk = Arc::new(FakeDesk::default());
        desk.windows.lock().unwrap().push(desk_window("old"));
        let browser = viewing(desk.clone());
        assert_eq!(browser.view("op").await["available"], false);
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        assert_eq!(browser.view("op").await["pending"], true, "a janela que já existia não entra");
        desk.windows.lock().unwrap().push(desk_window("new"));
        let frame = browser.view("op").await;
        assert_eq!(frame["title"], "Claude new");
        assert_eq!(frame["input"], true);
        browser.view("op").await;
        desk.windows.lock().unwrap().push(desk_window("later"));
        assert_eq!(browser.view("op").await["title"], "Claude new", "janela aberta depois não troca o alvo");
        assert_eq!(desk.log.lock().unwrap().as_slice(), ["present new"]);
        browser.close_view("op").await;
        assert_eq!(desk.log.lock().unwrap().last().unwrap(), "close new");
        assert_eq!(browser.view("op").await["available"], false);
    }

    #[tokio::test]
    async fn a_new_window_of_the_person_is_never_taken_for_the_login() {
        let desk = Arc::new(FakeDesk::default());
        let browser = viewing(desk.clone());
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        desk.windows.lock().unwrap().push(someone_elses_window("restored"));
        assert_eq!(browser.view("op").await["pending"], true, "janela restaurada pelo Chrome frio não é a do login");
        desk.windows.lock().unwrap().push(desk_window("login"));
        assert_eq!(browser.view("op").await["title"], "Claude login");
        browser.close_view("op").await;
        assert_eq!(desk.log.lock().unwrap().last().unwrap(), "close login");
    }

    #[tokio::test]
    async fn closing_finds_the_login_window_even_if_nobody_watched_it() {
        let desk = Arc::new(FakeDesk::default());
        let browser = viewing(desk.clone());
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        {
            let mut windows = desk.windows.lock().unwrap();
            windows.push(someone_elses_window("restored"));
            windows.push(desk_window("login"));
        }
        browser.close_view("op").await;
        assert_eq!(desk.log.lock().unwrap().as_slice(), ["close login"]);
    }

    #[tokio::test]
    async fn a_reused_window_address_is_not_closed() {
        let desk = Arc::new(FakeDesk::default());
        let browser = viewing(desk.clone());
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        desk.windows.lock().unwrap().push(desk_window("same"));
        browser.view("op").await;
        *desk.windows.lock().unwrap() = vec![someone_elses_window("same")];
        browser.close_view("op").await;
        assert!(!desk.log.lock().unwrap().iter().any(|line| line.starts_with("close")));
    }

    #[tokio::test]
    async fn input_that_did_not_arrive_is_an_error_and_a_live_session_is_not_taken_over() {
        let desk = Arc::new(FakeDesk::default());
        let browser = viewing(desk.clone());
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        desk.windows.lock().unwrap().push(desk_window("login"));
        browser.view("op").await;
        desk.input_fails.store(true, Ordering::Relaxed);
        assert_eq!(browser.input("op", DeskInput::Back).await.unwrap_err().code, "erro_login_view_input_failed");
        assert!(browser.activate("op").await);
        assert!(!desk.log.lock().unwrap().iter().any(|line| line == "activate"), "sessão ativa não usa tscon");
        desk.no_input.store(true, Ordering::Relaxed);
        assert!(browser.activate("op").await);
        assert!(desk.log.lock().unwrap().iter().any(|line| line == "activate"));
    }

    #[tokio::test]
    async fn default_browser_has_no_view_and_input_needs_a_live_session() {
        let desk = Arc::new(FakeDesk::default());
        let browser = viewing(desk.clone());
        browser.open(view_request(TargetKind::DefaultBrowser)).await.unwrap();
        assert_eq!(browser.view("op").await["available"], false);
        browser.open(view_request(TargetKind::ChromeProfile)).await.unwrap();
        let click = DeskInput::Back;
        assert_eq!(browser.input("op", click.clone()).await.unwrap_err().code, "erro_login_view_missing");
        desk.windows.lock().unwrap().push(desk_window("new"));
        browser.view("op").await;
        browser.input("op", click.clone()).await.unwrap();
        desk.no_input.store(true, Ordering::Relaxed);
        assert_eq!(browser.view("op").await["input"], false);
        assert_eq!(browser.input("op", click).await.unwrap_err().code, "erro_login_view_no_input");
        assert_eq!(browser.input("op", DeskInput::Text { text: "x".repeat(4097) }).await.unwrap_err().code, "erro_login_view_input_invalid");
        desk.unreadable.store(true, Ordering::Relaxed);
        let frame = browser.view("op").await;
        assert_eq!(frame["retry"], true, "lista que não veio não é janela fechada");
        assert!(frame["closed"].is_null());
        desk.unreadable.store(false, Ordering::Relaxed);
        desk.windows.lock().unwrap().clear();
        assert_eq!(browser.view("op").await["closed"], true);
    }

    #[test]
    fn view_routes_are_recognized_only_with_the_right_method() {
        use axum::http::Method;
        assert!(matches(&Method::GET, "/api/conta-estado/work/login/view"));
        assert!(!matches(&Method::POST, "/api/conta-estado/work/login/view"));
        assert!(matches(&Method::POST, "/api/conta-estado/work/login/view/input"));
        assert!(matches(&Method::POST, "/api/conta-estado/work/login/view/activate"));
        assert!(!matches(&Method::POST, "/api/conta-estado/work/login/view/other"));
        assert!(matches(&Method::GET, "/api/login-browser/view"));
        assert!(matches(&Method::POST, "/api/login-browser/input"));
    }

    #[test]
    fn empty_login_body_is_still_valid_and_broken_body_is_not() {
        assert_eq!(target_from_body(b""), Ok(None));
        assert_eq!(target_from_body(b"{}"), Ok(None));
        assert_eq!(target_from_body(b"{broken"), Err(()));
        let chosen = target_from_body(br#"{"target":{"machine":"m","kind":"chrome_profile","profile":"Default"}}"#);
        assert_eq!(chosen.unwrap().unwrap().profile.as_deref(), Some("Default"));
    }

    #[test]
    fn url_file_is_cleaned_of_quotes_and_line_ending() {
        assert_eq!(clean_url("\"https://x/?a=1&b=2\" \r\n"), "https://x/?a=1&b=2");
        assert_eq!(clean_url("https://x/\n"), "https://x/");
    }

    #[test]
    fn windows_script_is_ascii_and_writes_beside_itself() {
        let (name, content) = browser_script(Platform::Windows, "abc", "h.claude-login-url-abc").unwrap();
        assert_eq!(name, "claude-login-browser-abc.cmd");
        assert!(content.is_ascii());
        assert!(content.contains(r#">"%~dp0h.claude-login-url-abc" echo %*"#), "{content}");
        assert!(browser_script(Platform::Other, "abc", "x").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn linux_script_keeps_the_whole_link_even_with_a_space_in_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("a b");
        std::fs::create_dir(&folder).unwrap();
        let (name, content) = browser_script(Platform::Linux, "abc", &url_file_name("abc")).unwrap();
        let script = folder.join(name);
        write_browser_script(&script, &content).unwrap();
        let status = std::process::Command::new(&script).arg(LINK).status().unwrap();
        assert!(status.success());
        let saved = std::fs::read_to_string(folder.join(url_file_name("abc"))).unwrap();
        assert_eq!(clean_url(&saved), LINK);
        assert!(!folder.join("claude-login-url-abc.tmp").exists());
    }

    #[test]
    fn accepts_the_measured_link() {
        assert_eq!(
            parse_authorize_url(LINK),
            Some(AuthorizeLink { port: 34047, state: "st4te".into() })
        );
    }

    #[test]
    fn accepts_claude_ai_host() {
        let link = LINK.replacen("claude.com", "claude.ai", 1);
        assert_eq!(parse_authorize_url(&link).map(|l| l.port), Some(34047));
    }

    #[test]
    fn refuses_other_hosts_schemes_and_paths() {
        for link in [
            LINK.replacen("claude.com", "claude.com.evil.test", 1),
            LINK.replacen("claude.com", "evil.test", 1),
            LINK.replacen("https://", "http://", 1),
            LINK.replacen("/cai/oauth/authorize", "/cai/oauth/other", 1),
            LINK.replacen("https://claude.com", "https://user@claude.com", 1),
            LINK.replacen("https://claude.com", "https://claude.com:8443", 1),
        ] {
            assert_eq!(parse_authorize_url(&link), None, "{link}");
        }
    }

    #[test]
    fn refuses_any_other_redirect() {
        for redirect in [
            "https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback",
            "http%3A%2F%2Fevil.test%3A34047%2Fcallback",
            "http%3A%2F%2Flocalhost%2Fcallback",
            "http%3A%2F%2Flocalhost%3A0%2Fcallback",
            "http%3A%2F%2Flocalhost%3A70000%2Fcallback",
            "http%3A%2F%2Flocalhost%3A34047%2Fother",
            "http%3A%2F%2Flocalhost%3A34047%2Fcallback%3Fx%3D1",
            "https%3A%2F%2Flocalhost%3A34047%2Fcallback",
        ] {
            assert_eq!(parse_authorize_url(&with_redirect(redirect)), None, "{redirect}");
        }
    }

    #[test]
    fn refuses_missing_state_redirect_or_garbage() {
        let no_state = "https://claude.com/cai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A34047%2Fcallback";
        let empty_state = format!("{no_state}&state=");
        let no_redirect = "https://claude.com/cai/oauth/authorize?state=s";
        for link in [no_state, empty_state.as_str(), no_redirect, "", "not a url"] {
            assert_eq!(parse_authorize_url(link), None, "{link}");
        }
    }

    const LOCAL_STATE: &str = r#"{"profile":{"info_cache":{
        "Profile 1":{"name":"lhais","user_name":"lhais87.oliveira@gmail.com"},
        "Default":{"name":"Jefferson","user_name":"jeffer1312@gmail.com"},
        "Profile 2":{"name":"Trabalho","user_name":""},
        "Profile 3":{"name":"Sem conta"}
    }}}"#;

    fn profile(dir: &str, name: &str, email: Option<&str>) -> ChromeProfile {
        ChromeProfile { dir: dir.into(), name: name.into(), email: email.map(str::to_owned) }
    }

    #[test]
    fn lists_profiles_with_name_and_email_in_folder_order() {
        assert_eq!(
            list_chrome_profiles(LOCAL_STATE.as_bytes()),
            vec![
                profile("Default", "Jefferson", Some("jeffer1312@gmail.com")),
                profile("Profile 1", "lhais", Some("lhais87.oliveira@gmail.com")),
                profile("Profile 2", "Trabalho", None),
                profile("Profile 3", "Sem conta", None),
            ]
        );
    }

    #[test]
    fn unreadable_local_state_lists_nothing() {
        for raw in ["", "not json", "{}", r#"{"profile":{}}"#, r#"{"profile":{"info_cache":[]}}"#] {
            assert_eq!(list_chrome_profiles(raw.as_bytes()), Vec::new(), "{raw}");
        }
    }

    fn host(platform: Platform, chrome: bool, opener: bool) -> BrowserHost {
        BrowserHost {
            platform,
            chrome: chrome.then(|| PathBuf::from("/bin/chrome")),
            local_state: Some(LOCAL_STATE.as_bytes().to_vec()),
            default_opener: opener.then(|| PathBuf::from("/bin/open")),
        }
    }

    fn kinds(targets: &[Target]) -> Vec<(TargetKind, Option<&str>)> {
        targets.iter().map(|t| (t.kind, t.profile.as_deref())).collect()
    }

    #[test]
    fn linux_and_windows_offer_profiles_then_the_default_browser() {
        for platform in [Platform::Linux, Platform::Windows] {
            let targets = local_targets("pc", &host(platform, true, true));
            assert_eq!(
                kinds(&targets),
                vec![
                    (TargetKind::ChromeProfile, Some("Default")),
                    (TargetKind::ChromeProfile, Some("Profile 1")),
                    (TargetKind::ChromeProfile, Some("Profile 2")),
                    (TargetKind::ChromeProfile, Some("Profile 3")),
                    (TargetKind::DefaultBrowser, None),
                ]
            );
            assert!(targets.iter().all(|t| t.machine == "pc"));
            assert_eq!(targets[0].label, "Jefferson");
            assert_eq!(targets[0].email.as_deref(), Some("jeffer1312@gmail.com"));
        }
    }

    #[test]
    fn without_chrome_there_are_no_profiles() {
        let targets = local_targets("pc", &host(Platform::Linux, false, true));
        assert_eq!(kinds(&targets), vec![(TargetKind::DefaultBrowser, None)]);
    }

    #[test]
    fn without_an_opener_there_is_no_default_browser() {
        let targets = local_targets("pc", &host(Platform::Linux, true, false));
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().all(|t| t.kind == TargetKind::ChromeProfile));
    }

    #[test]
    fn other_platforms_offer_nothing() {
        assert_eq!(local_targets("pc", &host(Platform::Other, true, true)), Vec::new());
    }

    fn touch(file: &std::path::Path, content: &str) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    #[test]
    fn detects_a_linux_machine_from_path_and_home() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        touch(&bin.join("google-chrome-stable"), "");
        touch(&bin.join("xdg-open"), "");
        touch(&root.path().join(".config/google-chrome/Local State"), LOCAL_STATE);
        let (path, home) = (bin.display().to_string(), root.path().display().to_string());
        let found = detect_host_from(Platform::Linux, |key| match key {
            "PATH" => Some(path.clone()),
            "HOME" => Some(home.clone()),
            _ => None,
        });
        assert_eq!(found.chrome, Some(bin.join("google-chrome-stable")));
        assert_eq!(found.default_opener, Some(bin.join("xdg-open")));
        assert_eq!(local_targets("pc", &found).len(), 5);
    }

    #[test]
    fn detects_a_windows_machine_from_its_known_folders() {
        let root = tempfile::tempdir().unwrap();
        let (local, program, system) = (root.path().join("local"), root.path().join("pf"), root.path().join("win"));
        let chrome = program.join("Google").join("Chrome").join("Application").join("chrome.exe");
        touch(&chrome, "");
        touch(&local.join("Google").join("Chrome").join("User Data").join("Local State"), LOCAL_STATE);
        touch(&system.join("System32").join("rundll32.exe"), "");
        let text = |dir: &std::path::Path| dir.display().to_string();
        let found = detect_host_from(Platform::Windows, |key| match key {
            "LOCALAPPDATA" => Some(text(&local)),
            "ProgramFiles" => Some(text(&program)),
            "SystemRoot" => Some(text(&system)),
            _ => None,
        });
        assert_eq!(found.chrome, Some(chrome));
        assert_eq!(found.default_opener, Some(system.join("System32").join("rundll32.exe")));
        assert_eq!(local_targets("pc", &found).len(), 5);
    }

    #[test]
    fn a_machine_with_nothing_installed_offers_nothing() {
        let found = detect_host_from(Platform::Linux, |_| None);
        assert_eq!(local_targets("pc", &found), Vec::new());
    }

    fn target(machine: &str, kind: TargetKind, profile: Option<&str>) -> Target {
        Target { machine: machine.into(), kind, profile: profile.map(str::to_owned), label: String::new(), email: None }
    }

    #[test]
    fn chrome_opens_in_the_chosen_profile() {
        let command = browser_command(&host(Platform::Linux, true, true), &target("pc", TargetKind::ChromeProfile, Some("Profile 1")), LINK);
        assert_eq!(
            command,
            Some(BrowserCommand {
                program: PathBuf::from("/bin/chrome"),
                args: vec!["--profile-directory=Profile 1".to_owned(), "--new-window".to_owned(), LINK.to_owned()],
            })
        );
    }

    #[test]
    fn default_browser_differs_by_platform() {
        let default = target("pc", TargetKind::DefaultBrowser, None);
        let linux = browser_command(&host(Platform::Linux, true, true), &default, LINK).unwrap();
        assert_eq!((linux.program, linux.args), (PathBuf::from("/bin/open"), vec![LINK.to_owned()]));
        let windows = browser_command(&host(Platform::Windows, true, true), &default, LINK).unwrap();
        assert_eq!(windows.args, vec!["url.dll,FileProtocolHandler".to_owned(), LINK.to_owned()]);
    }

    #[test]
    fn a_profile_that_no_longer_exists_opens_nothing() {
        let linux = host(Platform::Linux, true, true);
        for profile in [Some("Profile 9"), Some("--no-sandbox"), None] {
            let gone = target("pc", TargetKind::ChromeProfile, profile);
            assert_eq!(browser_command(&linux, &gone, LINK), None, "{profile:?}");
        }
    }

    #[test]
    fn merge_names_each_target_by_the_peer_id_and_skips_who_did_not_answer() {
        let local = vec![target("", TargetKind::DefaultBrowser, None)];
        let peers = vec![
            ("off".to_owned(), None),
            ("nb".to_owned(), Some(vec![target("nome-la", TargetKind::DefaultBrowser, None)])),
        ];
        let (targets, last) = merge_targets(local, peers, None);
        let machines: Vec<&str> = targets.iter().map(|t| t.machine.as_str()).collect();
        assert_eq!(machines, ["", "nb"]);
        assert_eq!(last, None);
    }

    #[test]
    fn merge_keeps_last_only_while_it_is_still_offered() {
        let offered = target("nb", TargetKind::ChromeProfile, Some("Default"));
        let with_details = Target { label: "Jefferson".into(), email: Some("a@b.c".into()), ..offered.clone() };
        let peers = vec![("nb".to_owned(), Some(vec![with_details.clone()]))];
        assert_eq!(merge_targets(Vec::new(), peers.clone(), Some(offered)).1, Some(with_details));
        let gone = target("nb", TargetKind::ChromeProfile, Some("Profile 9"));
        assert_eq!(merge_targets(Vec::new(), peers, Some(gone)).1, None);
    }

    struct Rig {
        service: LoginBrowser,
        launched: Arc<Mutex<Vec<BrowserCommand>>>,
    }

    fn rig(fails: bool) -> Rig {
        let launched: Arc<Mutex<Vec<BrowserCommand>>> = Arc::default();
        let calls = launched.clone();
        let launcher: Launcher = Arc::new(move |command| {
            if fails {
                return Err(std::io::Error::other("sem tela"));
            }
            calls.lock().unwrap().push(command);
            Ok(())
        });
        let service = LoginBrowser::with(launcher, Arc::new(|| host(Platform::Linux, true, true)));
        Rig { service, launched }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn open_request(port: u16, remote: bool) -> OpenRequest {
        OpenRequest {
            url: LINK.replacen("34047", &port.to_string(), 1),
            operation: "op1".into(),
            target: target("pc", TargetKind::ChromeProfile, Some("Default")),
            remote,
        }
    }

    async fn send(port: u16, request: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    }

    async fn port_free(port: u16) -> bool {
        for _ in 0..200 {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn open_refuses_a_bad_link_or_target_without_launching() {
        let rig = rig(false);
        let mut bad_link = open_request(free_port(), false);
        bad_link.url = "https://evil.test/cai/oauth/authorize".into();
        assert_eq!(rig.service.open(bad_link).await.unwrap_err().code, "erro_login_link_refused");
        let mut bad_target = open_request(free_port(), false);
        bad_target.target = target("pc", TargetKind::ChromeProfile, Some("Profile 9"));
        assert_eq!(rig.service.open(bad_target).await.unwrap_err().code, "erro_login_target_missing");
        assert!(rig.launched.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn open_without_remote_launches_and_does_not_listen() {
        let (rig, port) = (rig(false), free_port());
        rig.service.open(open_request(port, false)).await.unwrap();
        assert_eq!(rig.launched.lock().unwrap().len(), 1);
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[tokio::test]
    async fn a_busy_port_is_reported_before_launching() {
        let rig = rig(false);
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        assert_eq!(rig.service.open(open_request(port, true)).await.unwrap_err().code, "erro_login_port_busy");
        assert!(rig.launched.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_launch_frees_the_port() {
        let (rig, port) = (rig(true), free_port());
        assert_eq!(rig.service.open(open_request(port, true)).await.unwrap_err().code, "erro_login_browser_failed");
        assert!(port_free(port).await);
        assert_eq!(rig.service.result("op1"), None);
    }

    #[tokio::test]
    async fn the_listener_keeps_only_the_right_callback_and_closes() {
        let (rig, port) = (rig(false), free_port());
        rig.service.open(open_request(port, true)).await.unwrap();
        assert_eq!(rig.service.result("op1"), None);

        assert!(send(port, get("/favicon.ico")).await.starts_with("HTTP/1.1 404"));
        assert!(send(port, get("/callback?code=abc&state=wrong")).await.starts_with("HTTP/1.1 404"));
        assert!(send(port, get("/callback?code=abc")).await.starts_with("HTTP/1.1 404"));
        let post = "POST /callback?code=abc&state=st4te HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        assert!(send(port, post.to_owned()).await.starts_with("HTTP/1.1 405"));
        assert_eq!(rig.service.result("op1"), None);

        let reply = send(port, get("/callback?code=abc&state=st4te")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(port_free(port).await);

        assert_eq!(rig.service.result("other"), None);
        assert_eq!(rig.service.result("op1").as_deref(), Some("code=abc&state=st4te"));
        assert_eq!(rig.service.result("op1"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_abandoned_listener_closes_at_the_cap() {
        let (rig, port) = (rig(false), free_port());
        rig.service.open(open_request(port, true)).await.unwrap();
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err());
        tokio::time::sleep(LISTEN_CAP + std::time::Duration::from_secs(1)).await;
        assert!(port_free(port).await);
    }

    #[tokio::test]
    async fn a_new_open_replaces_the_same_operation() {
        let (rig, first, second) = (rig(false), free_port(), free_port());
        rig.service.open(open_request(first, true)).await.unwrap();
        rig.service.open(open_request(second, true)).await.unwrap();
        assert!(port_free(first).await);
        assert!(std::net::TcpListener::bind(("127.0.0.1", second)).is_err());
    }

    #[test]
    fn only_the_contract_routes_match() {
        use axum::http::Method;
        for (method, path) in [
            (Method::GET, "/api/login-browser/targets"),
            (Method::GET, "/api/login-browser/result"),
            (Method::POST, "/api/login-browser/open"),
            (Method::GET, "/api/conta-estado/conta1/login/targets"),
        ] {
            assert!(matches(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::POST, "/api/login-browser/targets"),
            (Method::GET, "/api/login-browser/open"),
            (Method::GET, "/api/login-browser/other"),
            (Method::POST, "/api/conta-estado/conta1/login/targets"),
            (Method::GET, "/api/conta-estado//login/targets"),
            (Method::GET, "/api/conta-estado/a/b/login/targets"),
            (Method::GET, "/api/conta-estado/conta1/login/passo"),
        ] {
            assert!(!matches(&method, path), "{method} {path}");
        }
    }

    #[test]
    fn target_json_follows_the_contract() {
        let targets = local_targets("pc", &host(Platform::Linux, true, true));
        assert_eq!(
            serde_json::to_value(&targets[0]).unwrap(),
            serde_json::json!({"machine":"pc","kind":"chrome_profile","profile":"Default",
                "label":"Jefferson","email":"jeffer1312@gmail.com"})
        );
        let chosen: Target =
            serde_json::from_str(r#"{"machine":"pc","kind":"default_browser"}"#).unwrap();
        assert_eq!((chosen.kind, chosen.profile), (TargetKind::DefaultBrowser, None));
    }
}
