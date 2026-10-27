use super::*;
use crate::routes::AppState;
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

fn flag(env: &BTreeMap<String, String>, key: &str) -> bool {
    env.get(key).is_some_and(|s| {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "t" | "yes" | "y" | "on"
        )
    })
}
pub fn interval(env: &BTreeMap<String, String>) -> Result<f64, String> {
    let s = env
        .get("CP_OMP_PLUGIN_SYNC_INTERVAL")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    let n = if s.is_empty() {
        300.0
    } else {
        s.parse::<f64>()
            .map_err(|_| err("O intervalo deve ser positivo e finito"))?
    };
    if !n.is_finite() || n <= 0.0 {
        return Err(err("O intervalo deve ser positivo e finito"));
    }
    Ok(n)
}
/// Um produtor periódico por servidor; fechar aguarda a passagem em voo antes de devolver.
pub struct Service {
    sync: Synchronizer,
    enabled: bool,
    interval: f64,
    permitted: Arc<dyn Fn() -> bool + Send + Sync>,
    stopping: Arc<AtomicBool>,
    wake: Notify,
    status: Mutex<Value>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    ready: tokio::sync::watch::Sender<bool>,
}
impl Service {
    pub fn start(st: Arc<AppState>) -> Arc<Self> {
        let env = crate::push::server_env();
        let home = st.accounts.env.home.clone();
        let claude = st.accounts.env.claude_base.clone();
        let enabled = flag(&env, "CP_OMP_PLUGIN_SYNC_ENABLED");
        let context = flag(&env, "CP_OMP_CLAUDE_CONTEXT_ENABLED");
        let duration = interval(&env);
        let sync = Synchronizer::new(home.clone(), claude, env, Arc::new(NativeRunner));
        let weak = Arc::downgrade(&st);
        let allowed: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            weak.upgrade()
                .is_some_and(|st| crate::loops::automations(&st))
        });
        let service = Self::new(
            sync,
            enabled,
            duration.as_ref().copied().unwrap_or(300.0),
            allowed,
        )
        .expect("intervalo padrão válido");
        let clone = service.clone();
        let handle = tokio::spawn(async move {
            if context {
                let template = crate::install_root::root(&home)
                    .unwrap_or_default()
                    .join("scripts/omp/carregar-claude-md-do-projeto.md");
                let result = super::context::configure(&clone.sync, &template, true).await;
                if !result["errors"].as_array().is_some_and(Vec::is_empty) {
                    tracing::warn!(
                        code = "omp_context_configuration_failed",
                        "contexto OMP não configurado; preferências preservadas"
                    );
                }
            }
            clone.ready.send_replace(true);
            if let Err(e) = duration {
                let mut status = clone.status.lock().unwrap();
                status["state"] = json!("error");
                status["last_report"] = json!({"errors":[e]});
                return;
            }
            if enabled {
                clone.run().await
            }
        });
        *service.task.try_lock().expect("serviço novo sem disputa") = Some(handle);
        service
    }
    pub fn new(
        sync: Synchronizer,
        enabled: bool,
        interval: f64,
        permitted: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Arc<Self>, String> {
        if !interval.is_finite() || interval <= 0.0 {
            return Err(err("O intervalo deve ser positivo e finito"));
        }
        Ok(Arc::new(Self {
            sync,
            enabled,
            interval,
            permitted,
            stopping: Arc::new(AtomicBool::new(false)),
            wake: Notify::new(),
            status: Mutex::new(
                json!({"enabled":enabled,"state":if enabled{"idle"}else{"disabled"},"interval":interval,"started_at":null,"completed_at":null,"last_report":null}),
            ),
            task: tokio::sync::Mutex::new(None),
            ready: tokio::sync::watch::channel(false).0,
        }))
    }
    pub async fn ready(&self) {
        let mut receiver = self.ready.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn status(&self) -> Value {
        self.status.lock().unwrap().clone()
    }
    pub async fn start_loop(self: &Arc<Self>) {
        self.ready.send_replace(true);
        let mut task = self.task.lock().await;
        if self.enabled && task.is_none() && !self.stopping.load(Ordering::SeqCst) {
            let service = self.clone();
            *task = Some(tokio::spawn(async move { service.run().await }))
        }
    }
    pub async fn close(&self) {
        self.ready.send_replace(true);
        self.stopping.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        if let Some(task) = self.task.lock().await.take() {
            if let Err(e) = task.await {
                tracing::warn!(
                    code = "omp_sync_join_failed",
                    cancelled = e.is_cancelled(),
                    "sincronização OMP terminou sem confirmação de parada"
                );
                self.status.lock().unwrap()["state"] = json!("error");
                return;
            }
            self.status.lock().unwrap()["state"] = json!("stopped");
        }
    }
    async fn run(&self) {
        while !self.stopping.load(Ordering::SeqCst) {
            if !(self.permitted)() {
                self.status.lock().unwrap()["state"] = json!("paused")
            } else {
                {
                    let mut s = self.status.lock().unwrap();
                    s["state"] = json!("running");
                    s["started_at"] = json!(crate::nav_requests::now());
                }
                let interrupted = AtomicBool::new(false);
                let stop = || {
                    if self.stopping.load(Ordering::SeqCst) || !(self.permitted)() {
                        interrupted.store(true, Ordering::SeqCst)
                    }
                    interrupted.load(Ordering::SeqCst)
                };
                let mut reports = Map::new();
                if !stop() {
                    reports.insert(
                        "marketplaces".into(),
                        self.sync.import_marketplaces(false, &stop).await,
                    );
                }
                if !stop() {
                    reports.insert("plugins".into(), self.sync.reconcile(false, &stop).await);
                }
                let state = if self.stopping.load(Ordering::SeqCst) {
                    "stopped"
                } else if interrupted.load(Ordering::SeqCst) || !(self.permitted)() {
                    "paused"
                } else if reports.values().any(|r| {
                    r["errors"].as_array().is_some_and(|a| !a.is_empty())
                        || r["complete_inventory"] != true
                }) {
                    "error"
                } else {
                    let actions = reports
                        .values()
                        .filter_map(|r| r["items"].as_array())
                        .flatten()
                        .filter_map(|i| i["action"].as_str())
                        .collect::<BTreeSet<_>>();
                    if actions.contains("suspended") || actions.contains("conflict") {
                        "suspended"
                    } else if [
                        "install",
                        "uninstall",
                        "enable",
                        "disable",
                        "adopt",
                        "import",
                    ]
                    .iter()
                    .any(|a| actions.contains(a))
                    {
                        "updated"
                    } else {
                        "unchanged"
                    }
                };
                let mut s = self.status.lock().unwrap();
                s["state"] = json!(state);
                s["completed_at"] = json!(crate::nav_requests::now());
                s["last_report"] = json!(reports);
            }
            if !self.stopping.load(Ordering::SeqCst) {
                let mut remaining = self.interval;
                while remaining > 0.0 && !self.stopping.load(Ordering::SeqCst) {
                    // O Python aceita qualquer float finito; um prazo enorme não cabe no Instant.
                    let step = remaining.min(86400.0);
                    tokio::select! { _ = self.wake.notified() => break, _ = tokio::time::sleep(Duration::from_secs_f64(step)) => remaining -= step }
                }
            }
        }
    }
}
pub fn matches(method: &Method, path: &str) -> bool {
    method == Method::GET && path == "/api/omp/plugin-sync"
}
/// A borda já conferiu o dono, inclusive nas portas repassadas.
pub async fn serve(_st: &Arc<AppState>, service: &Arc<Service>, _req: Request) -> Response {
    crate::session_write::json_response(StatusCode::OK, service.status())
}
