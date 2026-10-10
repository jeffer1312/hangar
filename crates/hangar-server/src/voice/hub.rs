//! Uma chamada por servidor; o aparelho que abriu por último é o dono. Sem dono, a chamada espera um prazo e encerra.
use super::call::{CallOptions, Spawn, Voice};
use super::controller::{Controller, DeviceLink, GateSource};
use super::machines::{Machines, SelfApi};
use super::organizer::tools_for;
use super::protocol::{ClientMsg, Screen, ServerMsg, ToController};
use super::rpc::Rpc;
use super::settings::{self, VOICES, VoiceSettings};
use crate::accounts::AccountService;
use crate::routes::AppState;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::Duration;
use tokio::sync::mpsc;

/// Sobe um app-server por chamada; o teste troca pelo falso.
pub type SpawnFactory = Arc<dyn Fn() -> Spawn + Send + Sync>;

const GRACE: Duration = Duration::from_secs(120);
const GATE_EVERY: Duration = Duration::from_secs(5);

/// O que o teste injeta: pastas próprias, prazos curtos, app-server falso e contas fixas.
pub struct TestParts {
    pub home: PathBuf, pub claude_dir: PathBuf, pub grace: Duration, pub gate_every: Duration,
    pub spawn: Option<SpawnFactory>, pub accounts: Option<Vec<String>>,
}

struct LiveCall {
    call_id: u64,
    /// Dono atual; muda a cada aparelho que assume.
    epoch: u64,
    client: String,
    /// Fica aqui a chamada inteira: o controlador encerra quando o canal fecha.
    to_controller: mpsc::UnboundedSender<ToController>,
    link: DeviceLink,
}

pub struct VoiceHub {
    home: PathBuf,
    claude_dir: PathBuf,
    grace: Duration,
    gate_every: Duration,
    /// Contas do teste; `None` pergunta ao catálogo.
    accounts: Option<Vec<String>>,
    spawn: Option<SpawnFactory>,
    /// A API pública deste servidor; gravada quando o listener sobe.
    own: OnceLock<SelfApi>,
    live: Mutex<Option<LiveCall>>,
    /// Dois aparelhos abrindo juntos não sobem duas chamadas.
    opening: tokio::sync::Mutex<()>,
    next: AtomicU64,
}

/// O que a conexão do aparelho recebe ao virar dono.
pub struct Attached { pub epoch: u64, pub from_call: mpsc::UnboundedReceiver<ServerMsg> }

/// O `hello` do aparelho.
pub struct Hello { pub client: String, pub screen: Option<Screen>, pub caps: Vec<String>, pub actions: Vec<Value> }

impl Default for VoiceHub {
    fn default() -> Self { Self::new() }
}

impl VoiceHub {
    pub fn new() -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        let claude_dir = settings::claude_dir(&home);
        Self::with_parts(TestParts { home, claude_dir, grace: GRACE, gate_every: GATE_EVERY, spawn: None, accounts: None })
    }

    pub fn with_parts(p: TestParts) -> Self {
        Self { home: p.home, claude_dir: p.claude_dir, grace: p.grace, gate_every: p.gate_every, accounts: p.accounts, spawn: p.spawn,
            own: OnceLock::new(), live: Mutex::new(None), opening: tokio::sync::Mutex::new(()), next: AtomicU64::new(0) }
    }

    pub fn home(&self) -> &Path { &self.home }

    pub fn claude_dir(&self) -> &Path { &self.claude_dir }

    /// `Account.id` é o nome que as sessões gravam em `codex_account`.
    pub async fn account_ids(&self, accounts: &AccountService) -> Vec<String> {
        if let Some(ids) = &self.accounts { return ids.clone(); }
        let accounts = accounts.clone();
        match tokio::task::spawn_blocking(move || accounts.visible_codex_accounts()).await {
            Ok(Ok(list)) => list.into_iter().map(|a| a.id).collect(),
            Ok(Err(e)) => { super::log(format!("voice codex accounts unreadable status={}", e.status)); Vec::new() }
            Err(_) => { super::log("voice codex accounts read panicked"); Vec::new() }
        }
    }

    pub fn set_self(&self, addr: SocketAddr, token: &str) { let _ = self.own.set(SelfApi::new(addr, token)); }

    pub fn self_api(&self) -> Option<SelfApi> { self.own.get().cloned() }

    fn lock(&self) -> MutexGuard<'_, Option<LiveCall>> { self.live.lock().unwrap_or_else(|e| e.into_inner()) }

    /// `(ativa, cliente dono)`.
    pub fn call_status(&self) -> (bool, Option<String>) {
        match &*self.lock() { Some(call) => (true, Some(call.client.clone())), None => (false, None) }
    }

    /// Modelos novos valem já; voz e conta, na próxima chamada.
    pub fn apply_settings(&self, chosen: &VoiceSettings) {
        if let Some(call) = &*self.lock() { let _ = call.to_controller.send(ToController::Models(chosen.organizer.clone())); }
    }

    /// O aparelho vira dono: assume a chamada viva ou abre uma. `Err` é o código que vai ao aparelho.
    pub async fn attach(self: &Arc<Self>, st: &AppState, hello: Hello) -> Result<Attached, &'static str> {
        let _opening = self.opening.lock().await;
        let (owner, from_call) = mpsc::unbounded_channel();
        let epoch = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let Hello { client, screen, caps, actions } = hello;
        let device_hello = ClientMsg::Hello { client: client.clone(), screen: screen.clone(), caps: caps.clone(), actions };
        {
            let mut live = self.lock();
            if let Some(call) = live.as_mut().filter(|c| !c.to_controller.is_closed()) {
                log_owner("voice call taken over", &client);
                if let Some(old) = call.link.replace_owner(owner, caps, screen) { let _ = old.send(ServerMsg::Taken); }
                (call.epoch, call.client) = (epoch, client);
                // A oferta que o aparelho manda em seguida vira a passagem da conversa falada para ele.
                let _ = call.to_controller.send(ToController::OwnerChanged);
                let _ = call.to_controller.send(ToController::Device(device_hello));
                return Ok(Attached { epoch, from_call });
            }
            *live = None;
        }
        let (home, claude_dir) = (self.home.clone(), self.claude_dir.clone());
        let (gate, chosen) = tokio::task::spawn_blocking(move || (settings::read_gate(&home, &claude_dir), settings::read_settings(&home))).await
            .map_err(|_| "failed")?;
        if !gate.enabled { return Err("disabled"); }
        let spawn = self.spawn_for(st, &chosen.codex_account).await?;
        let Some(own) = self.self_api() else { super::log("voice refused: own api not ready"); return Err("failed") };
        let name = screen.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let context = if name.is_empty() { "Nenhuma sessão aberta na tela.".to_owned() } else { format!("A sessão na tela agora é {name}.") };
        let options = CallOptions { voice: chosen.voice.filter(|v| VOICES.contains(&v.as_str())), context, cwd: None, target: name,
            organizer: chosen.organizer, tools: tools_for(&caps), handoff_same_thread: true };
        let (events_tx, events) = async_channel::unbounded();
        let voice = Voice::start(options, spawn, events_tx);
        let own_label = st.groups.as_ref().map(|g| g.server_id().to_owned()).filter(|id| !id.is_empty()).unwrap_or_else(|| "este servidor".to_owned());
        let machines = Arc::new(Machines { own, peers: st.peers.clone(), own_label });
        let link = DeviceLink::default();
        link.replace_owner(owner, caps, screen);
        let gate_source = GateSource { home: self.home.clone(), claude_dir: self.claude_dir.clone(), every: self.gate_every };
        let controller = Controller::new(voice, machines, gate.jev, link.clone(), gate_source, st.diag.clone());
        let (to_controller, from_device) = mpsc::unbounded_channel();
        let _ = to_controller.send(ToController::Device(device_hello));
        let call_id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        log_owner("voice call opened", &client);
        *self.lock() = Some(LiveCall { call_id, epoch, client, to_controller, link });
        let hub = self.clone();
        tokio::spawn(async move { controller.run(events, from_device).await; hub.ended(call_id); });
        Ok(Attached { epoch, from_call })
    }

    /// Mensagem do aparelho: só a do dono atual chega à chamada. `false` = quem mandou já não é o dono.
    pub fn forward(&self, epoch: u64, msg: ClientMsg) -> bool {
        match &*self.lock() {
            Some(call) if call.epoch == epoch => { let _ = call.to_controller.send(ToController::Device(msg)); true }
            _ => { super::log("voice message from a former owner dropped"); false }
        }
    }

    /// A conexão do dono caiu: a chamada espera outro aparelho pelo prazo e então encerra.
    pub fn detach(self: &Arc<Self>, epoch: u64) {
        let mut live = self.lock();
        let Some(call) = live.as_mut().filter(|c| c.epoch == epoch) else { return };
        call.link.clear_owner();
        let _ = call.to_controller.send(ToController::Detached);
        let (hub, call_id, grace) = (self.clone(), call.call_id, self.grace);
        super::log("voice owner detached: grace started");
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let mut live = hub.lock();
            if live.as_ref().is_some_and(|c| c.call_id == call_id && c.epoch == epoch) {
                super::log("voice grace over: stopping the call");
                if let Some(call) = live.take() { let _ = call.to_controller.send(ToController::Device(ClientMsg::Stop)); }
            }
        });
    }

    /// O controlador terminou. O fim atrasado de uma chamada velha nunca limpa a nova.
    fn ended(&self, call_id: u64) {
        let mut live = self.lock();
        if live.as_ref().is_some_and(|c| c.call_id == call_id) && let Some(call) = live.take() {
            super::log("voice call ended");
            call.link.send(ServerMsg::Closed);
        }
    }

    /// O app-server da conta escolhida. Conta que sumiu recusa, nunca cai na padrão.
    async fn spawn_for(&self, st: &AppState, account_id: &str) -> Result<Spawn, &'static str> {
        if let Some(ids) = &self.accounts && !ids.iter().any(|id| id == account_id) { return Err("account_missing"); }
        if let Some(factory) = &self.spawn { return Ok(factory()); }
        let (accounts, id) = (st.accounts.clone(), account_id.to_owned());
        let command = tokio::task::spawn_blocking(move || -> Result<tokio::process::Command, &'static str> {
            let list = accounts.visible_codex_accounts().map_err(|e| {
                super::log(format!("voice codex accounts unreadable status={}", e.status));
                "account_missing"
            })?;
            let account = list.into_iter().find(|a| a.id == id).ok_or("account_missing")?;
            let mut command = accounts.codex_command().ok_or("codex_missing")?;
            if !account.is_default { command.args(["-c", r#"cli_auth_credentials_store="file""#]); }
            command.arg("app-server").env_clear().envs(accounts.env.codex(&account));
            Ok(command)
        }).await.map_err(|_| "failed")??;
        // Largar o `Rpc` (fim da chamada) derruba o app-server e o grupo dele.
        Ok(Box::new(move || Box::pin(Rpc::spawn(command))))
    }
}

/// Só o tipo de cliente (`pwa`, `native`), nunca texto do aparelho além disso.
fn log_owner(what: &str, client: &str) {
    let client = if matches!(client, "pwa" | "native") { client } else { "other" };
    super::log(format!("{what} client={client}"));
}
