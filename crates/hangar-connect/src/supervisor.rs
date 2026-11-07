//! `frpc` e Caddy como filhos do servidor: sobem na ordem certa, voltam depois de cair e morrem com ele.
//!
//! No Mac nada mata o filho junto com o pai: quem sobrou é encerrado por `kill_leftover` na volta do Hangar.
use std::ffi::{OsStr, OsString};
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};

use super::devices::Store;
use super::peers::Links;
use super::preview::PREVIEW_PORT;
use super::state::{self, LinkRequest, Linked};
use super::{CONNECT_PORT, ConnectError, GUEST_PORT, binaries, config};
use hangar_core::terminal_process::{CommandTree, OwnedTree, PRIVATE_ENV_KEYS};

const FRPC_WAITS: [u64; 5] = [1, 2, 5, 10, 30];
/// Cada subida do Caddy sem certificado pode gastar uma validação do limite semanal, que é de todas as máquinas.
const CADDY_WAITS: [u64; 5] = [5, 30, 60, 300, 900];
const HEALTHY: Duration = Duration::from_secs(60);
const STOP_GRACE: Duration = Duration::from_secs(5);
const LOGIN_PATIENCE: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_millis(250);
const LOGIN_LINE: &[u8] = b"login to server success";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Os dois filhos de pé.
    pub enabled: bool,
    /// A supervisão ligada, mesmo com um filho subindo ou esperando para voltar.
    pub running: bool,
    pub host: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
struct ChildState {
    running: bool,
    restarts: usize,
    last_exit: Option<i32>,
}

#[derive(Default)]
struct Live {
    host: Option<String>,
    active: bool,
    frpc: ChildState,
    caddy: ChildState,
    port_error: Option<String>,
    start_error: Option<String>,
    login_error: Option<String>,
}

type Shared = Arc<Mutex<Live>>;

fn lock(live: &Shared) -> MutexGuard<'_, Live> {
    live.lock().unwrap_or_else(|e| e.into_inner())
}

fn child_state<'a>(live: &'a mut Live, name: &str) -> &'a mut ChildState {
    if name == "frpc" { &mut live.frpc } else { &mut live.caddy }
}

struct Spec {
    name: &'static str,
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    log: PathBuf,
    pid_file: PathBuf,
    waits: &'static [u64],
}

struct Run {
    stop: watch::Sender<bool>,
    done: oneshot::Receiver<()>,
}

pub struct Supervisor {
    dir: PathBuf,
    log_dir: PathBuf,
    main_port: u16,
    devices: Arc<Store>,
    programs: Option<(PathBuf, PathBuf)>,
    live: Shared,
    /// A entrada do Connect aberta por este servidor.
    bound: watch::Sender<bool>,
    /// A porta do convidado aberta por este servidor.
    guest_port: watch::Sender<bool>,
    /// A porta da prévia aberta por este servidor.
    preview_port: watch::Sender<bool>,
    /// A subida do servidor e um pedido da tela podem chegar juntos: sem fila, nasceriam dois de cada.
    run: tokio::sync::Mutex<Option<Run>>,
    linking: tokio::sync::Mutex<()>,
    peer_links: Links,
}

fn main_port_message() -> String {
    format!("CP_PORT={CONNECT_PORT} é a porta do Connect")
}

fn start_failed(cause: String) -> ConnectError {
    ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_start_failed", format!("o Connect não subiu: {cause}"))
}

pub(super) async fn off_thread<T: Send + 'static>(op: impl FnOnce() -> Result<T, ConnectError> + Send + 'static) -> Result<T, ConnectError> {
    tokio::task::spawn_blocking(op).await.unwrap_or_else(|cause| Err(start_failed(cause.to_string())))
}

impl Supervisor {
    pub fn new(dir: PathBuf, log_dir: PathBuf, devices: Arc<Store>, main_port: u16) -> Self {
        let live = Live { port_error: (main_port == CONNECT_PORT).then(main_port_message), ..Live::default() };
        Self { dir, log_dir, main_port, devices, programs: None, live: Arc::new(Mutex::new(live)), bound: watch::channel(false).0,
            guest_port: watch::channel(false).0, preview_port: watch::channel(false).0, run: tokio::sync::Mutex::new(None), linking: tokio::sync::Mutex::new(()), peer_links: Links::default() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn peer_links(&self) -> &Links {
        &self.peer_links
    }

    /// Teste: programas falsos no lugar dos baixados.
    #[doc(hidden)]
    pub fn with_programs(mut self, frpc: PathBuf, caddy: PathBuf) -> Self {
        self.programs = Some((frpc, caddy));
        self
    }

    fn port_refusal(&self) -> Option<ConnectError> {
        (self.main_port == CONNECT_PORT).then(|| ConnectError::new(StatusCode::CONFLICT, "connect_port_is_main", main_port_message()))
    }

    pub fn port_opened(&self) {
        lock(&self.live).port_error = None;
        self.bound.send_replace(true);
    }

    /// Com a porta na mão de outro processo, o Caddy entregaria a internet a ele.
    pub fn port_failed(&self, error: String) {
        lock(&self.live).port_error = Some(error);
        self.bound.send_replace(false);
    }

    pub fn guest_port_opened(&self) {
        self.guest_port.send_replace(true);
    }

    pub fn preview_port_opened(&self) {
        self.preview_port.send_replace(true);
    }

    pub async fn linked(&self) -> Option<Linked> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || state::read(&dir)).await.ok().flatten()
    }

    pub fn status(&self) -> Status {
        let live = lock(&self.live);
        let falling: Vec<String> = [("frpc", &live.frpc), ("caddy", &live.caddy)].into_iter()
            .filter(|(_, child)| child.restarts >= 3)
            .map(|(name, child)| {
                let exit = child.last_exit.map_or_else(|| "desconhecida".to_owned(), |code| code.to_string());
                format!("{name} caiu {} vezes (saída {exit})", child.restarts)
            })
            .collect();
        let error = live.port_error.clone().or_else(|| live.start_error.clone()).or_else(|| live.login_error.clone())
            .or_else(|| (!falling.is_empty()).then(|| format!("{}; veja connect-<nome>.log na pasta de logs do Hangar", falling.join("; "))));
        Status { enabled: live.frpc.running && live.caddy.running, running: live.active, host: live.host.clone(), error }
    }

    async fn halt(&self, run: &mut Option<Run>) {
        if let Some(Run { stop, done }) = run.take() {
            let _ = stop.send(true);
            let _ = done.await;
        }
        let mut live = lock(&self.live);
        live.active = false;
        live.login_error = None;
        live.frpc = ChildState::default();
        live.caddy = ChildState::default();
    }

    async fn programs(&self) -> Result<(PathBuf, PathBuf), ConnectError> {
        if let Some(programs) = &self.programs {
            return Ok(programs.clone());
        }
        let [frpc, caddy] = binaries::local()?;
        Ok((binaries::fetch(&frpc, &self.dir).await?, binaries::fetch(&caddy, &self.dir).await?))
    }

    fn spec(&self, name: &'static str, program: PathBuf, args: Vec<OsString>, waits: &'static [u64]) -> Spec {
        Spec { name, program, args, waits, cwd: self.dir.clone(), log: self.log_dir.join(format!("connect-{name}.log")),
            pid_file: pid_file(&self.dir, name) }
    }

    async fn launch(&self, linked: &Linked) -> Result<Run, ConnectError> {
        if let Some(refusal) = self.port_refusal() {
            return Err(refusal);
        }
        let dir = self.dir.clone();
        off_thread(move || state::protect(&dir)).await?;
        let (frpc, caddy) = self.programs().await?;
        for name in ["frpc", "caddy"] {
            kill_leftover(&self.dir, name).await?;
        }
        let (frpc_config, caddyfile) = (self.dir.join("frpc.toml"), self.dir.join("Caddyfile"));
        let frpc_text = config::render_frpc(linked)?;
        let caddy_text = config::render_caddyfile(linked, &self.dir)?;
        let sites = Sites { file: caddyfile.clone(), owner: caddy_text.clone(),
            guest: linked.guest_published.then(|| Site::new(self.guest_port.subscribe(), config::render_guest_site(linked))),
            preview: linked.preview_published.then(|| Site::new(self.preview_port.subscribe(), config::render_preview_site(linked))) };
        let (toml_path, caddy_path) = (frpc_config.clone(), caddyfile.clone());
        off_thread(move || {
            state::write_private(&toml_path, frpc_text.as_bytes())?;
            state::write_private(&caddy_path, caddy_text.as_bytes())
        }).await?;
        let frpc = self.spec("frpc", frpc, vec!["-c".into(), frpc_config.into_os_string()], &FRPC_WAITS);
        let caddy = self.spec("caddy", caddy, vec!["run".into(), "--config".into(), caddyfile.into_os_string(),
            "--adapter".into(), "caddyfile".into()], &CADDY_WAITS);
        let (stop, stop_rx) = watch::channel(false);
        let (done_tx, done) = oneshot::channel();
        let (live, bound) = (self.live.clone(), self.bound.subscribe());
        lock(&self.live).active = true;
        // O sinal de morte do Linux segue a thread que criou o filho, e um worker do tokio pode sair antes
        // (`block_in_place`): esta thread vive tanto quanto os filhos.
        std::thread::Builder::new().name("connect-supervisor".to_owned()).spawn(move || {
            supervise_on_thread(frpc, caddy, sites, live, stop_rx, bound);
            let _ = done_tx.send(());
        }).map_err(|cause| {
            lock(&self.live).active = false;
            start_failed(cause.to_string())
        })?;
        Ok(Run { stop, done })
    }

    /// Falha fica também em `status().error`: a subida do servidor não tem quem leia o retorno.
    pub async fn start(&self, linked: &Linked) -> Result<(), ConnectError> {
        let mut run = self.run.lock().await;
        self.halt(&mut run).await;
        {
            let mut live = lock(&self.live);
            live.host = Some(linked.host.clone());
            live.start_error = None;
        }
        match self.launch(linked).await {
            Ok(started) => {
                *run = Some(started);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(code = error.code, "o Connect não subiu: {}", error.msg);
                lock(&self.live).start_error = Some(error.msg.clone());
                Err(error)
            }
        }
    }

    pub async fn stop(&self) {
        let mut run = self.run.lock().await;
        self.halt(&mut run).await;
    }

    /// Volta da ligação gravada, na subida do servidor.
    pub async fn resume(&self) {
        // Desligar entre a leitura e a subida religaria o que acabou de ser apagado.
        let _linking = self.linking.lock().await;
        let dir = self.dir.clone();
        let Ok(Some(linked)) = tokio::task::spawn_blocking(move || state::read(&dir)).await else { return };
        match linked.identity() {
            Ok(identity) => self.devices.set_identity(Some(identity)),
            Err(error) => tracing::warn!(code = error.code, "identidade do Connect ilegível"),
        }
        // A causa fica em `status().error`.
        let _ = self.start(&linked).await;
    }

    /// Falha ao subir os filhos devolve o erro, mas a ligação fica gravada: o serviço já trocou a chave.
    /// Quem pede de novo a mesma ligação com a supervisão parada só tenta subir outra vez.
    pub async fn link(&self, request: LinkRequest) -> Result<(), ConnectError> {
        let _linking = self.linking.lock().await;
        if let Some(refusal) = self.port_refusal() {
            return Err(refusal);
        }
        let dir = self.dir.clone();
        if let Some(saved) = off_thread(move || Ok(state::read(&dir))).await? {
            let same = saved.account == request.account && saved.machine_id == request.machine_name;
            if !same || lock(&self.live).active {
                return Err(ConnectError::new(StatusCode::CONFLICT, "connect_already_linked", "esta máquina já está ligada ao Connect"));
            }
            self.devices.set_identity(Some(saved.identity()?));
            return self.start(&saved).await;
        }
        let linked = state::register(&request).await?;
        let (dir, saved) = (self.dir.clone(), linked.clone());
        off_thread(move || state::write(&dir, &saved)).await?;
        self.devices.set_identity(Some(linked.identity()?));
        self.start(&linked).await
    }

    /// Primeiro uso de um site: o `frpc` e o Caddy só leem a configuração ao subir, então os dois voltam uma vez.
    /// Gravado antes da troca, nada repete isso. Cancelada no meio deixaria o Connect parado: quem chama usa tarefa própria.
    async fn publish(&self, port: &watch::Sender<bool>, closed: ConnectError, published: fn(&mut Linked) -> &mut bool) -> Result<Linked, ConnectError> {
        let _linking = self.linking.lock().await;
        if !*port.borrow() {
            return Err(closed);
        }
        let dir = self.dir.clone();
        let Some(mut linked) = off_thread(move || Ok(state::read(&dir))).await? else {
            return Err(ConnectError::new(StatusCode::CONFLICT, "connect_not_linked", "esta máquina não está ligada ao Connect"));
        };
        if *published(&mut linked) {
            return Ok(linked);
        }
        *published(&mut linked) = true;
        let (dir, saved) = (self.dir.clone(), linked.clone());
        off_thread(move || state::write(&dir, &saved)).await?;
        self.start(&linked).await?;
        Ok(linked)
    }

    pub async fn publish_guest(&self) -> Result<(), ConnectError> {
        let closed = ConnectError::new(StatusCode::CONFLICT, "connect_guest_port_closed", format!("a porta do convite ({GUEST_PORT}) não abriu neste servidor"));
        self.publish(&self.guest_port, closed, |linked| &mut linked.guest_published).await.map(|_| ())
    }

    /// Devolve o host da prévia.
    pub async fn publish_preview(&self) -> Result<String, ConnectError> {
        let closed = ConnectError::new(StatusCode::CONFLICT, "connect_preview_port_closed", format!("a porta da prévia ({PREVIEW_PORT}) não abriu neste servidor"));
        Ok(self.publish(&self.preview_port, closed, |linked| &mut linked.preview_published).await?.preview_host)
    }

    /// Credencial de aparelho fica: ela vale também no Tailscale.
    pub async fn forget(&self) -> Result<(), ConnectError> {
        let _linking = self.linking.lock().await;
        let mut run = self.run.lock().await;
        // Desligado primeiro no disco: se parar os filhos falhar, a próxima subida não religa.
        let dir = self.dir.clone();
        off_thread(move || state::remove(&dir)).await?;
        self.devices.set_identity(None);
        self.halt(&mut run).await;
        // Subida que falhou antes da supervisão deixa de pé o filho de uma vida anterior.
        let frpc = kill_leftover(&self.dir, "frpc").await;
        let caddy = kill_leftover(&self.dir, "caddy").await;
        let dir = self.dir.clone();
        let removed = off_thread(move || state::remove_files(&dir)).await;
        {
            let mut live = lock(&self.live);
            let port_error = live.port_error.take();
            *live = Live { port_error, ..Live::default() };
        }
        frpc.and(caddy).and(removed)
    }
}

fn pid_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.pid"))
}

fn supervise_on_thread(frpc: Spec, caddy: Spec, sites: Sites, live: Shared, stop: watch::Receiver<bool>, bound: watch::Receiver<bool>) {
    let panicked = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => std::panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(supervise(frpc, caddy, sites, live.clone(), stop, bound)))).is_err(),
        Err(_) => true,
    };
    let mut live = lock(&live);
    live.active = false;
    live.frpc.running = false;
    live.caddy.running = false;
    if panicked {
        live.start_error = Some("o supervisor do Connect parou".to_owned());
    }
}

async fn supervise(frpc: Spec, caddy: Spec, sites: Sites, live: Shared, stop: watch::Receiver<bool>, bound: watch::Receiver<bool>) {
    let (logged, logged_rx) = watch::channel(false);
    let (site_stop, site_live) = (stop.clone(), live.clone());
    tokio::join!(
        keep(frpc, live, Some(logged), None, stop),
        keep(caddy, site_live, None, Some(Gate { logged: logged_rx, bound, sites }), site_stop),
    );
}

/// O que o Caddy espera antes de nascer: pedido sem túnel é validação perdida, e sem a 8768 aberta
/// por este servidor ele entregaria a internet a quem a segura.
struct Gate {
    logged: watch::Receiver<bool>,
    bound: watch::Receiver<bool>,
    sites: Sites,
}

/// Site já publicado, que só entra no Caddyfile com a porta dele aberta por este servidor: com ela na
/// mão de outro processo, o Caddy entregaria a internet a ele.
struct Site {
    open: watch::Receiver<bool>,
    served: bool,
    text: String,
}

impl Site {
    fn new(open: watch::Receiver<bool>, text: String) -> Self {
        Self { open, served: false, text }
    }

    /// Resolve uma vez só: com o servidor de pé a porta não fecha, e o Caddy seguinte já sobe com o site.
    async fn until_missing(&mut self) {
        if self.served || self.open.wait_for(|on| *on).await.is_err() {
            std::future::pending().await
        }
    }
}

async fn until_opened(site: Option<&mut Site>) {
    match site {
        Some(site) => site.until_missing().await,
        None => std::future::pending().await,
    }
}

/// O Caddyfile sai a cada subida com o site do dono e os publicados de porta aberta.
/// O Caddy não espera por elas: a entrada do dono não fica presa a uma porta que outro processo segura.
struct Sites {
    file: PathBuf,
    owner: String,
    guest: Option<Site>,
    preview: Option<Site>,
}

impl Sites {
    fn write(&mut self) -> Result<(), ConnectError> {
        let mut text = self.owner.clone();
        for site in [self.guest.as_mut(), self.preview.as_mut()].into_iter().flatten() {
            site.served = *site.open.borrow();
            if site.served {
                text.push('\n');
                text.push_str(&site.text);
            }
        }
        state::write_private(&self.file, text.as_bytes())
    }

    async fn until_missing(&mut self) {
        tokio::select! {
            () = until_opened(self.guest.as_mut()) => {}
            () = until_opened(self.preview.as_mut()) => {}
        }
    }
}

impl Gate {
    /// `false` é pedido de parada.
    async fn opened(&mut self, stop: &mut watch::Receiver<bool>) -> bool {
        let ready = async {
            loop {
                self.bound.wait_for(|on| *on).await?;
                self.logged.wait_for(|on| *on).await?;
                if *self.bound.borrow() {
                    return Ok::<(), watch::error::RecvError>(());
                }
            }
        };
        tokio::select! {
            ready = ready => ready.is_ok(),
            _ = stop.changed() => false,
        }
    }
}

async fn until_port_lost(bound: Option<&mut watch::Receiver<bool>>) {
    match bound {
        Some(bound) => { let _ = bound.wait_for(|on| !*on).await; }
        None => std::future::pending().await,
    }
}

async fn until_site_missing(site: Option<&mut Sites>) {
    match site {
        Some(site) => site.until_missing().await,
        None => std::future::pending().await,
    }
}

enum Ended {
    Stopped,
    PortLost,
    SiteOpened,
    Exited(Option<i32>),
    Failed,
}

async fn keep(spec: Spec, live: Shared, logged: Option<watch::Sender<bool>>, mut gate: Option<Gate>, mut stop: watch::Receiver<bool>) {
    loop {
        if let Some(gate) = &mut gate && !gate.opened(&mut stop).await {
            return;
        }
        let (bound, mut site) = match gate.as_mut() {
            Some(Gate { bound, sites, .. }) => (Some(bound), Some(sites)),
            None => (None, None),
        };
        let ended = match site.as_mut().map_or(Ok(()), |site| site.write()) {
            Ok(()) => run_once(&spec, &live, logged.as_ref(), bound, site, &mut stop).await,
            Err(error) => {
                tracing::warn!(code = error.code, child = spec.name, "filho do Connect sem configuração: {}", error.msg);
                Ended::Failed
            }
        };
        // O próximo Caddy espera um login novo; o que está de pé fica, porque cada subida custa validação.
        if let Some(logged) = &logged {
            logged.send_replace(false);
        }
        let (wait, exit) = {
            let mut live = lock(&live);
            let child = child_state(&mut live, spec.name);
            child.running = false;
            let exit = match ended {
                Ended::Stopped => return,
                Ended::PortLost | Ended::SiteOpened => continue,
                Ended::Exited(code) => code,
                Ended::Failed => None,
            };
            let wait = spec.waits[child.restarts.min(spec.waits.len() - 1)];
            child.restarts += 1;
            child.last_exit = exit;
            (wait, exit)
        };
        tracing::warn!(code = "connect_child_down", child = spec.name, ?exit, wait, "filho do Connect caiu; volta depois da espera");
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(wait)) => {}
            _ = stop.changed() => return,
        }
    }
}

async fn run_once(spec: &Spec, live: &Shared, logged: Option<&watch::Sender<bool>>, bound: Option<&mut watch::Receiver<bool>>,
    site: Option<&mut Sites>, stop: &mut watch::Receiver<bool>) -> Ended {
    let (mut child, mut tree, mut offset) = match spawn_child(spec) {
        Ok(started) => started,
        Err(cause) => {
            tracing::warn!(code = "connect_child_failed", child = spec.name, "filho do Connect não subiu: {cause}");
            return Ended::Failed;
        }
    };
    child_state(&mut lock(live), spec.name).running = true;
    let started = Instant::now();
    let mut waiting = logged.is_some();
    let mut healthy = false;
    let mut tick = tokio::time::interval(TICK);
    let lost = until_port_lost(bound);
    tokio::pin!(lost);
    let missing = until_site_missing(site);
    tokio::pin!(missing);
    loop {
        tokio::select! {
            status = child.wait() => return match status {
                Ok(status) => Ended::Exited(status.code()),
                Err(cause) => {
                    tracing::warn!(code = "connect_child_failed", child = spec.name, "filho do Connect sem estado: {cause}");
                    Ended::Failed
                }
            },
            _ = stop.changed() => {
                stop_child(spec.name, &mut child, &mut tree).await;
                return Ended::Stopped;
            }
            () = &mut lost => {
                tracing::warn!(code = "connect_port_lost", child = spec.name, "a entrada do Connect saiu deste servidor; filho parado");
                stop_child(spec.name, &mut child, &mut tree).await;
                return Ended::PortLost;
            }
            () = &mut missing => {
                tracing::info!(code = "connect_site_opened", child = spec.name, "a porta de um site abriu; filho volta com ele");
                stop_child(spec.name, &mut child, &mut tree).await;
                return Ended::SiteOpened;
            }
            _ = tick.tick() => {
                if waiting && let Some(logged) = logged {
                    if scan_login(&spec.log, &mut offset).unwrap_or(false) {
                        waiting = false;
                        logged.send_replace(true);
                        lock(live).login_error = None;
                    } else if started.elapsed() >= LOGIN_PATIENCE {
                        lock(live).login_error = Some("o frpc não entrou no serviço do Connect; veja connect-frpc.log na pasta de logs do Hangar".to_owned());
                    }
                }
                if !healthy && started.elapsed() >= HEALTHY {
                    healthy = true;
                    child_state(&mut lock(live), spec.name).restarts = 0;
                }
            }
        }
    }
}

fn open_log(path: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // shortcut: gira só na subida do filho; quem fica meses de pé passa do teto até a próxima.
    hangar_core::log_rotate::rotate_log(path, hangar_core::log_rotate::LOG_MAX_BYTES);
    // Acréscimo: num laço de quedas, a causa da anterior continua no arquivo.
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

fn spawn_child(spec: &Spec) -> io::Result<(Child, CommandTree, u64)> {
    let log = open_log(&spec.log)?;
    let offset = log.metadata()?.len();
    let mut command = Command::new(&spec.program);
    // Pasta própria: no Windows, processo vivo com a pasta da versão como `cwd` impede a poda dela.
    command.args(&spec.args).current_dir(&spec.cwd).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).kill_on_drop(true);
    for key in PRIVATE_ENV_KEYS {
        command.env_remove(key);
    }
    let mut tree = CommandTree::configure(&mut command)?;
    die_with_parent(&mut command);
    let mut child = command.spawn()?;
    if let Err(error) = tree.attach(&child) {
        let _ = child.start_kill();
        return Err(error);
    }
    if let Some(pid) = child.id() && let Err(error) = std::fs::write(&spec.pid_file, pid.to_string()) {
        tracing::warn!(code = "connect_pid_not_written", child = spec.name, "pid do filho não gravado: {error}");
    }
    Ok((child, tree, offset))
}

#[cfg(target_os = "linux")]
fn die_with_parent(command: &mut Command) {
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: entre o fork e o exec só há chamadas que o fork permite.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                return Err(io::Error::last_os_error());
            }
            // Pai morto antes do `prctl`: o sinal nunca viria.
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

// No Windows quem leva o filho é o Job do `CommandTree`.
#[cfg(not(target_os = "linux"))]
fn die_with_parent(_command: &mut Command) {}

/// Linha pela metade fica para a próxima leitura.
fn scan_login(path: &Path, offset: &mut u64) -> io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(*offset))?;
    let mut text = Vec::new();
    file.read_to_end(&mut text)?;
    let whole = text.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    *offset += whole as u64;
    Ok(memchr::memmem::find(&text[..whole], LOGIN_LINE).is_some())
}

async fn stop_child(name: &'static str, child: &mut Child, tree: &mut CommandTree) {
    // Falha ao matar não para a supervisão: o outro filho ficaria sem dono.
    if let Err(error) = ask_to_leave(child, tree) {
        tracing::warn!(code = "connect_stop_failed", child = name, "filho do Connect não recebeu o aviso de saída: {error}");
    }
    if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
        return;
    }
    tracing::warn!(code = "connect_stop_forced", child = name, "filho do Connect não saiu no prazo; forçando");
    if let Err(error) = tree.terminate() {
        tracing::warn!(code = "connect_stop_failed", child = name, "filho do Connect não foi forçado: {error}");
    }
    if let Err(error) = child.kill().await {
        tracing::warn!(code = "connect_stop_failed", child = name, "filho do Connect não foi forçado: {error}");
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) -> io::Result<()> {
    // SAFETY: só manda sinal; o filho nasce como líder do próprio grupo.
    if unsafe { libc::kill(-(pid as i32), signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) }
}

#[cfg(unix)]
fn ask_to_leave(child: &Child, _tree: &mut CommandTree) -> io::Result<()> {
    child.id().map_or(Ok(()), |pid| signal_group(pid, libc::SIGTERM))
}

#[cfg(windows)]
fn ask_to_leave(_child: &Child, tree: &mut CommandTree) -> io::Result<()> {
    tree.terminate()
}

fn executable(pid: u32) -> Option<PathBuf> {
    let mut system = System::new();
    let target = Pid::from_u32(pid);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[target]), true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always));
    system.process(target)?.exe().map(Path::to_path_buf)
}

/// Pid reaproveitado pode ser qualquer coisa: só é nosso o que roda de `bin/<versão>/<nome>`.
fn ours(dir: &Path, name: &str, pid: u32) -> bool {
    let Some(exe) = executable(pid) else { return false };
    let (Ok(exe), Ok(bin)) = (exe.canonicalize(), dir.join("bin").canonicalize()) else { return false };
    exe.file_stem() == Some(OsStr::new(name)) && exe.parent().and_then(Path::parent) == Some(bin.as_path())
}

fn leftover_failed(name: &str, cause: impl std::fmt::Display) -> ConnectError {
    ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_leftover_alive", format!("o {name} de antes não saiu: {cause}"))
}

#[cfg(unix)]
async fn end_leftover(name: &str, pid: u32, force: bool) -> Result<(), ConnectError> {
    signal_group(pid, if force { libc::SIGKILL } else { libc::SIGTERM }).map_err(|cause| leftover_failed(name, cause))
}

/// O código de retorno do `taskkill` não decide (128 = já morreu): quem decide é a conferência depois.
#[cfg(windows)]
async fn end_leftover(name: &str, pid: u32, _force: bool) -> Result<(), ConnectError> {
    let system = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32").join("taskkill.exe"))
        .filter(|path| path.is_file());
    let mut command = Command::new(system.unwrap_or_else(|| PathBuf::from("taskkill")));
    command.args(["/T", "/F", "/PID", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(0x08000000);
    match command.status().await {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR,
            "connect_taskkill_missing", format!("taskkill não encontrado: o {name} de antes segue de pé"))),
        Err(error) => Err(leftover_failed(name, error)),
    }
}

/// Filho de uma vida anterior seguraria a porta local e o nome no `frps`.
pub async fn kill_leftover(dir: &Path, name: &str) -> Result<(), ConnectError> {
    let Ok(text) = std::fs::read_to_string(pid_file(dir, name)) else { return Ok(()) };
    let Ok(pid) = text.trim().parse::<u32>() else { return Ok(()) };
    if !ours(dir, name, pid) {
        return Ok(());
    }
    end_leftover(name, pid, false).await?;
    let mut deadline = Instant::now() + STOP_GRACE;
    let mut forced = false;
    while ours(dir, name, pid) {
        if Instant::now() >= deadline {
            if forced {
                return Err(leftover_failed(name, "continua de pé"));
            }
            end_leftover(name, pid, true).await?;
            forced = true;
            deadline = Instant::now() + STOP_GRACE;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}
