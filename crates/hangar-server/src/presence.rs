//! "No PC" / "Fora": com o dono no PC o push do celular fica retido. A escolha (`mode`) é do dono e
//! sobrevive a reinício; estar no PC de fato exige o sinal de vida do app de desktop com a janela à vista.
//! À parte, cada app nativo manda um relatório de si (versão, memória, CPU) a cada 30 s, também na bandeja,
//! para o bloco Máquinas da tela Status.
use crate::routes::{AppState, gate, pass};
use crate::session_write::{detail_body, json_response};
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use hangar_api::status::AppReport;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Dois sinais de 30 s perdidos: queda, PC desligado ou app fechado sem aviso viram "Fora".
const ALIVE_FOR: Duration = Duration::from_secs(75);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode { Pc, Away }

impl Mode {
    fn parse(text: &str) -> Option<Self> {
        match text { "pc" => Some(Self::Pc), "away" => Some(Self::Away), _ => None }
    }
    fn as_str(self) -> &'static str { if self == Self::Pc { "pc" } else { "away" } }
}

struct Presence { mode: Option<Mode>, seen: Option<Instant> }

static STATE: Mutex<Presence> = Mutex::new(Presence { mode: None, seen: None });

/// O último relatório de um app nativo.
#[derive(Clone, Debug, PartialEq)]
pub struct AppSeen {
    /// O `device-id` do aparelho: um relatório por aparelho, nunca por `host`.
    pub device: String,
    /// Identificador do servidor da máquina onde o app roda; ausente quando ele não tem servidor local.
    pub machine_id: Option<String>,
    pub report: AppReport,
}

/// Por `device`, com a hora do último relatório.
struct Apps(BTreeMap<String, (AppSeen, Instant)>);

impl Apps {
    const fn new() -> Self { Self(BTreeMap::new()) }

    fn record(&mut self, app: AppSeen, at: Instant) {
        self.0.retain(|_, (_, seen)| alive_at(*seen, at));
        self.0.insert(app.device.clone(), (app, at));
    }

    fn alive(&self, now: Instant) -> Vec<AppSeen> {
        self.0.values().filter(|(_, seen)| alive_at(*seen, now)).map(|(app, _)| app.clone()).collect()
    }
}

static APPS: Mutex<Apps> = Mutex::new(Apps::new());

fn alive_at(seen: Instant, now: Instant) -> bool { now.saturating_duration_since(seen) < ALIVE_FOR }

/// Os apps nativos que mandaram relatório nos últimos `ALIVE_FOR` (75 s), em ordem de `device`.
pub fn apps(now: Instant) -> Vec<AppSeen> { APPS.lock().unwrap_or_else(|e| e.into_inner()).alive(now) }

/// Corpo de `POST /api/presence/app`; campo obrigatório ausente ou fora de forma recusa o relatório inteiro.
fn parse_app(body: &Value) -> Option<AppSeen> {
    let text = |key: &str| body[key].as_str().map(str::trim).filter(|t| !t.is_empty() && t.len() <= 128).map(str::to_owned);
    let report = AppReport {
        version: text("app_version")?,
        rss_bytes: body["rss_bytes"].as_u64()?,
        cpu_pct: body["cpu_pct"].as_f64().filter(|v| v.is_finite() && *v >= 0.0)?,
        platform: text("platform")?,
        host: text("host")?,
    };
    Some(AppSeen { device: text("device")?, machine_id: text("machine_id"), report })
}

fn file(state: &AppState) -> PathBuf { state.accounts.env.home.join(".hangar").join("presenca.json") }

/// Sem arquivo ou arquivo torto: "No PC", o padrão que só retém push com o app aberto e à vista.
fn stored(path: &Path) -> Mode {
    std::fs::read(path).ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v["mode"].as_str().and_then(Mode::parse))
        .unwrap_or(Mode::Pc)
}

fn view(state: &AppState, now: Instant) -> Value {
    let mut p = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let mode = *p.mode.get_or_insert_with(|| stored(&file(state)));
    let alive = p.seen.is_some_and(|at| now.saturating_duration_since(at) < ALIVE_FOR);
    json!({"mode": mode.as_str(), "desktop_alive": alive, "present": mode == Mode::Pc && alive})
}

/// O produtor de avisos consulta em processo: só `true` retém o envio.
pub fn present(state: &AppState) -> bool { view(state, Instant::now())["present"] == true }

/// `GET /api/presence`, `POST /api/presence {"mode"}`, `POST /api/presence/heartbeat {"leaving"?}` e
/// `POST /api/presence/app {device, machine_id?, app_version, platform, host, rss_bytes, cpu_pct}`; só o dono.
pub async fn route(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, request: Request) -> Response {
    let (forward, owner) = match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner {
        return pass(&state, request, &forward).await;
    }
    let heartbeat = request.uri().path().ends_with("/heartbeat");
    let app = request.uri().path().ends_with("/presence/app");
    if request.method() == Method::GET && !heartbeat && !app {
        return json_response(StatusCode::OK, view(&state, Instant::now()));
    }
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let body = to_bytes(request.into_body(), 4 * 1024).await.ok()
        .and_then(|bytes| if bytes.is_empty() { Some(json!({})) } else { serde_json::from_slice::<Value>(&bytes).ok() });
    let Some(body) = body.filter(Value::is_object) else { return invalid() };
    if app {
        let Some(seen) = parse_app(&body) else {
            return json_response(StatusCode::BAD_REQUEST, detail_body("presence_app_invalid", "relatório do app inválido", json!({})));
        };
        APPS.lock().unwrap_or_else(|e| e.into_inner()).record(seen, Instant::now());
        return json_response(StatusCode::OK, json!({"ok": true}));
    }
    if heartbeat {
        let mut p = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // A janela saiu de vista ou o app fechou: o celular volta a receber na hora, sem esperar o prazo.
        p.seen = if body["leaving"] == true { None } else { Some(Instant::now()) };
        drop(p);
        return json_response(StatusCode::OK, view(&state, Instant::now()));
    }
    let Some(mode) = body["mode"].as_str().and_then(Mode::parse) else { return invalid() };
    let path = file(&state);
    let bytes = json!({"mode": mode.as_str()}).to_string();
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| crate::runtime::queue::atomic_write(&path, bytes.as_bytes()));
    if written.is_err() {
        return json_response(StatusCode::INTERNAL_SERVER_ERROR,
            detail_body("presence_write_failed", "não consegui gravar o status de presença", json!({})));
    }
    STATE.lock().unwrap_or_else(|e| e.into_inner()).mode = Some(mode);
    json_response(StatusCode::OK, view(&state, Instant::now()))
}

fn invalid() -> Response {
    json_response(StatusCode::BAD_REQUEST, detail_body("presence_invalid", "status de presença inválido: use pc ou away", json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_mode_defaults_to_pc_and_reads_away() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("presenca.json");
        assert_eq!(stored(&path), Mode::Pc);
        std::fs::write(&path, r#"{"mode":"away"}"#).unwrap();
        assert_eq!(stored(&path), Mode::Away);
        std::fs::write(&path, "torto").unwrap();
        assert_eq!(stored(&path), Mode::Pc);
    }

    fn report(device: &str, host: &str) -> AppSeen {
        AppSeen { device: device.into(), machine_id: Some("casa".into()),
            report: AppReport { version: "1.2.3".into(), rss_bytes: 1, cpu_pct: 0.5, platform: "linux".into(), host: host.into() } }
    }

    #[test]
    fn presence_app_forgets_a_device_silent_for_76s_and_keeps_another_with_the_same_host() {
        let t0 = Instant::now();
        let mut apps = Apps::new();
        apps.record(report("a", "notebook"), t0);
        apps.record(report("b", "notebook"), t0 + Duration::from_secs(40));
        assert_eq!(apps.alive(t0 + Duration::from_secs(74)).len(), 2);
        assert_eq!(apps.alive(t0 + Duration::from_secs(76)), vec![report("b", "notebook")]);
    }

    #[test]
    fn presence_app_new_report_of_the_same_device_replaces_the_old() {
        let t0 = Instant::now();
        let mut apps = Apps::new();
        apps.record(report("a", "notebook"), t0);
        apps.record(report("a", "casa"), t0 + Duration::from_secs(30));
        assert_eq!(apps.alive(t0 + Duration::from_secs(90)), vec![report("a", "casa")]);
    }

    #[test]
    fn presence_app_body_needs_every_field_and_machine_id_is_optional() {
        let full = json!({"device": "a", "machine_id": "casa", "app_version": "1.2.3", "platform": "linux", "host": "notebook",
            "rss_bytes": 1, "cpu_pct": 0.5});
        assert_eq!(parse_app(&full), Some(report("a", "notebook")));
        let mut no_machine = full.clone();
        no_machine.as_object_mut().unwrap().remove("machine_id");
        assert_eq!(parse_app(&no_machine).unwrap().machine_id, None);
        for key in ["device", "app_version", "platform", "host", "rss_bytes", "cpu_pct"] {
            let mut body = full.clone();
            body.as_object_mut().unwrap().remove(key);
            assert_eq!(parse_app(&body), None, "{key}");
        }
        let mut negative = full.clone();
        negative["cpu_pct"] = json!(-1.0);
        assert_eq!(parse_app(&negative), None);
        let mut blank = full;
        blank["device"] = json!("  ");
        assert_eq!(parse_app(&blank), None);
    }

    #[test]
    fn only_known_modes_parse() {
        assert_eq!(Mode::parse("pc"), Some(Mode::Pc));
        assert_eq!(Mode::parse("away"), Some(Mode::Away));
        assert_eq!(Mode::parse("PC"), None);
    }
}
