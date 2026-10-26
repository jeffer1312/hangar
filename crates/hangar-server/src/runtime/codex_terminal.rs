//! Codex com terminal adotado: a TUI segue no tmux e o Rust se conecta, por WebSocket de loopback, ao
//! app-server que o lançador subiu. Abrir sessão nova é do Python; o resto é daqui: a vigia (`Watcher`)
//! adota pelo sidecar e pelo pane, segue a identidade nova, fecha na morte do pane, encerra pelo app e
//! troca de modo (`codex_relaunch`). Nada aqui sobe app-server.
use super::cano::IoTasks;
use super::codex_sidecar::{self,SidecarLife};
use super::gateway::RuntimeRegistry;
use super::protocol::{CanoBinding,ClockSample,RuntimeError,RuntimeTarget};
use super::queue::{Action,CODEX_LIFE,State as QueueState,Store,acquire_lease};
use super::websocket::WebSocketConnection;
use crate::list::discover_other::truthy;
use crate::list::mux::{Mux,Pane};
use serde_json::{Map,Value,json};
use std::collections::{BTreeMap,BTreeSet};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path,PathBuf};
use std::sync::{Arc,Mutex};
use std::time::Duration;
use tokio::sync::{Notify,mpsc,watch};
pub use super::cano_reconnect::Attempt;
use super::cano_reconnect::ReconnectWaits;
#[cfg(test)]
use super::cano_reconnect::CanoReconnect;

/// Problema da sessão quando o app-server saiu ou mudou (`Gone`): pede a pessoa. Código já traduzido em
/// todas as telas; o que aconteceu vai no detalhe.
pub const DISCONNECTED:&str = "runtime_falhou";
/// O socket de controle caiu e o ator religa ao mesmo app-server: a TUI segue respondendo, então é
/// diagnóstico (`problem_gate::diagnostic`), nunca aviso na tela.
pub const CONTROL_LOST:&str = "codex_control_lost";

#[derive(Clone,Debug,PartialEq)]
pub struct CodexTerminalTarget {
    pub key: String,
    pub generation: u64,
    pub name: String,
    pub endpoint: String,
    pub app_pid: u32,
    pub thread_id: String,
    pub terminal: crate::terminal_input::TerminalBinding,
    pub metadata: serde_json::Value,
    pub transcript: Option<std::path::PathBuf>,
    pub lease_path: std::path::PathBuf,
    pub state_path: std::path::PathBuf,
    pub projection_dir: std::path::PathBuf,
    /// Pasta dos sidecars: com ela o ator grava o sidecar desta vida sem o Python.
    pub sidecar_dir: Option<std::path::PathBuf>,
}

impl CodexTerminalTarget {
    /// Mesma vida: o que identifica o processo, o pane e os arquivos. A conversa fica de fora: quem a troca
    /// é o ator, seguindo a TUI, sem trocar a sessão.
    pub fn same_life(&self,other:&Self) -> bool {
        let pane = |t:&crate::terminal_input::TerminalBinding|(t.name.clone(),t.pane.clone(),t.generation,t.created,t.mux_argv.clone(),t.windows);
        (self.generation,&self.endpoint,self.app_pid,pane(&self.terminal),&self.lease_path,&self.state_path,&self.projection_dir)
            == (other.generation,&other.endpoint,other.app_pid,pane(&other.terminal),&other.lease_path,&other.state_path,&other.projection_dir)
    }

    /// O sidecar desta vida, para o ator gravar o `session.patch_meta` localmente.
    pub fn sidecar_life(&self) -> Option<SidecarLife> {
        self.sidecar_dir.clone().map(|dir|SidecarLife { dir,name:self.name.clone(),key:self.key.clone(),endpoint:self.endpoint.clone(),
            app_pid:self.app_pid,thread:Arc::new(Mutex::new(self.thread_id.clone())) })
    }

    /// Metadata do motor: sempre com terminal, na conversa e na chave do descritor.
    pub fn engine_metadata(&self) -> Value {
        let mut metadata = if self.metadata.is_object() { self.metadata.clone() } else { json!({}) };
        metadata["headless"] = json!(false);
        metadata["name"] = json!(self.name);
        metadata["key"] = json!(self.key);
        metadata["thread_id"] = json!(self.thread_id);
        metadata
    }

    /// Alvo do ator. Sem cano: pid 0 é "nenhum", e sem `launch` o ator nunca sobe nem mata nada.
    pub fn runtime_target(&self) -> RuntimeTarget {
        RuntimeTarget { key:self.key.clone(),generation:self.generation,name:self.name.clone(),provider:"codex".into(),
            metadata:self.engine_metadata(),binding:CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 },
            lease_path:self.lease_path.clone(),state_path:self.state_path.clone(),projection_dir:self.projection_dir.clone(),
            // Rollout ausente é válido: o motor aprende o caminho depois do primeiro turno.
            transcript:self.transcript.clone().unwrap_or_default(),
            created:self.metadata["created"].as_f64().unwrap_or(self.terminal.created as f64) }
    }
}

/// Só `ws://<IP de loopback literal>:<porta>`. Nome nunca é resolvido: nenhum descritor leva o Rust a outra máquina.
pub fn loopback_endpoint(endpoint:&str) -> Result<SocketAddr,RuntimeError> {
    let refused = ||RuntimeError::new("codex_endpoint","endereço do app-server não é local");
    let address:SocketAddr = endpoint.strip_prefix("ws://").ok_or_else(refused)?.parse().map_err(|_|refused())?;
    if !address.ip().is_loopback() || address.port() == 0 { return Err(refused()); }
    Ok(address)
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum AppServer { Ours, Dead, Foreign }

/// O mesmo critério do `matar_app_server` do Python: o pid tem que ser o `app-server` deste endereço.
pub fn app_server(pid:u32,endpoint:&str) -> AppServer {
    match super::process::argv(pid) {
        None => AppServer::Dead,
        Some(argv) if argv.iter().any(|a|a == "app-server") && argv.iter().any(|a|a == endpoint) => AppServer::Ours,
        Some(_) => AppServer::Foreign,
    }
}

async fn app_server_of(pid:u32,endpoint:&str) -> AppServer {
    let endpoint = endpoint.to_owned();
    tokio::task::spawn_blocking(move||app_server(pid,&endpoint)).await.unwrap_or(AppServer::Dead)
}

/// Admissão: endereço local, conversa identificada e o pid comprovado como o app-server do endereço.
pub async fn verify(target:&CodexTerminalTarget) -> Result<(),RuntimeError> {
    loopback_endpoint(&target.endpoint)?;
    if target.thread_id.is_empty() { return Err(RuntimeError::new("codex_thread","Codex com terminal sem conversa identificada")); }
    match app_server_of(target.app_pid,&target.endpoint).await {
        AppServer::Ours => Ok(()),
        _ => Err(RuntimeError::new("codex_app_pid","o pid informado não é o app-server deste endereço")),
    }
}

/// Vínculo adotado como o registro o tem agora; `None` depois de fechar ou transferir a sessão. Leva
/// também quem acordar quando o app-server some (a vigia) e o sidecar que o ator grava.
#[derive(Clone,Default)]
pub struct AdoptionCell { current:Arc<Mutex<Option<(String,u32)>>>, wake:Option<Arc<Notify>>, sidecar:Option<SidecarLife>,
    /// Trava do pane desta vida: `/keys`, `/term-input` e o seletor de permissões não se intercalam.
    pane:Arc<tokio::sync::Mutex<()>> }

impl AdoptionCell {
    pub fn new(endpoint:&str,app_pid:u32) -> Self { Self { current:Arc::new(Mutex::new(Some((endpoint.to_owned(),app_pid)))),wake:None,sidecar:None,pane:Default::default() } }
    pub fn pane_serial(&self) -> Arc<tokio::sync::Mutex<()>> { self.pane.clone() }
    pub fn with_lifecycle(mut self,wake:Arc<Notify>,sidecar:Option<SidecarLife>) -> Self { self.wake = Some(wake); self.sidecar = sidecar; self }
    pub fn current(&self) -> Option<(String,u32)> { self.current.lock().unwrap_or_else(|e|e.into_inner()).clone() }
    /// Antes de ceder a posse: tentativa em voo que termine depois disto não vale.
    pub fn revoke(&self) { *self.current.lock().unwrap_or_else(|e|e.into_inner()) = None; }
    /// O app-server saiu ou trocou: a vigia olha agora, sem esperar a rodada.
    pub fn wake(&self) { if let Some(wake) = &self.wake { wake.notify_one(); } }
    pub fn sidecar(&self) -> Option<&SidecarLife> { self.sidecar.as_ref() }
}

/// O que o ator precisa para reconectar sem o registro: o endereço e o pid da abertura, e a célula viva.
#[derive(Clone)]
pub struct Adoption { pub endpoint:String, pub app_pid:u32, pub cell:AdoptionCell }

impl Adoption {
    fn still_ours(&self) -> bool { self.cell.current().is_some_and(|(endpoint,pid)|endpoint == self.endpoint && pid == self.app_pid) }
}

/// Uma tentativa: revalida vínculo, endereço e processo antes de conectar, e o vínculo de novo depois.
/// `Gone` (vínculo trocado, sessão fechada ou processo morto) acorda a vigia, que adota a vida nova ou
/// fecha esta; nenhum app-server é subido daqui. Nunca `Stopped`.
pub async fn reconnect(adoption:&Adoption) -> Attempt {
    if !adoption.still_ours() { return Attempt::Gone(RuntimeError::new("codex_adoption_changed","a sessão trocou de dono ou de app-server")); }
    if let Err(error) = loopback_endpoint(&adoption.endpoint) { return Attempt::Gone(error); }
    if app_server_of(adoption.app_pid,&adoption.endpoint).await != AppServer::Ours {
        return Attempt::Gone(RuntimeError::new("codex_app_server_gone","o app-server da sessão não está mais vivo neste endereço"));
    }
    match WebSocketConnection::connect(&adoption.endpoint).await {
        Ok(_) if !adoption.still_ours() => Attempt::Gone(RuntimeError::new("codex_adoption_changed","a sessão trocou de dono durante a reconexão")),
        Ok(connection) => Attempt::Connected(super::actor::RuntimeConnection::WebSocket(connection)),
        Err(error) => Attempt::Retry(error),
    }
}

/// Esperas da reconexão ao app-server adotado: 1 s, ×1,5, até 10 s, com ou sem mensagem esperando.
pub(crate) fn reconnect_waits() -> ReconnectWaits {
    let normal:Vec<Duration> = [1.0,1.5,2.25,3.375,5.0625,7.59375,10.0].into_iter().map(Duration::from_secs_f64).collect();
    ReconnectWaits { pending:normal.clone(),normal,..ReconnectWaits::default() }
}

/// Transporte que já caiu: escrever nele falha na hora e ele não produz eventos.
pub(super) fn closed_io() -> IoTasks {
    let (writer,_) = mpsc::channel(1);
    let (ui_writer,_) = mpsc::channel(1);
    let (_,events) = mpsc::channel(1);
    IoTasks { writer,ui_writer,events,stop:watch::channel(false).0,reader_task:tokio::spawn(async {}),writer_task:tokio::spawn(async {}) }
}

/// Rodada da vigia: o `_watch_tmux` do Python também olhava a cada 1 s.
const WATCH_EVERY:Duration = Duration::from_secs(1);
/// Tempo sem o pane e com a TUI morta antes de dar a sessão por encerrada. Por tempo, não por rodadas:
/// o `wake` dispara rodadas a milissegundos uma da outra.
const DEATH_AFTER:Duration = Duration::from_secs(3);

/// Onde a vigia lê e grava. `queue_dir` é o `_queue_dir` do Python (`<conta Claude>/.hangar-queue`).
pub struct WatchConfig { pub home:PathBuf, pub queue_dir:PathBuf, pub mux:Mux, pub mux_program:OsString }

/// Vigia do Codex com terminal: adota, segue renomear e app-server ressubido, fecha na morte do pane e
/// encerra pelo app. Só existe quando a tabela entrega o Codex com terminal ao Rust.
pub struct Watcher {
    pub(super) cfg:WatchConfig,
    /// Desde quando cada vida parece morta; segura também a exclusão entre rodada, adoção pedida pela
    /// escrita, encerramento e troca de modo.
    pub(super) misses:tokio::sync::Mutex<BTreeMap<String,std::time::Instant>>,
    death_after:Duration,
    /// Relançamentos seguidos que falharam do sidecar antigo, por nome, com a hora da última falha (o
    /// `TETO_SUBIDAS` do Python).
    pub(super) legacy_failures:tokio::sync::Mutex<BTreeMap<String,(u8,std::time::Instant)>>,
    /// O prefixo do escopo do systemd do pane que o Rust cria; os testes nunca rodam o `systemd-run`.
    scope_probe:fn()->Vec<String>,
}

/// Sidecar do desenho antigo (terminal sem `endpoint`): a escrita o relança pelo lançador.
pub(super) const LEGACY_SIDECAR:&str = "codex_terminal_legacy";

/// Sidecar do Codex com terminal pronto para adotar.
#[derive(Clone,Debug)]
pub(super) struct Candidate { name:String, meta:Map<String,Value>, endpoint:String, app_pid:u32, thread_id:String, key:Option<String>, tui_pid:Option<u32> }

pub(super) fn text<'a>(meta:&'a Map<String,Value>,field:&str) -> Option<&'a str> { meta.get(field).and_then(Value::as_str).filter(|s|!s.is_empty()) }

pub(super) fn pid(meta:&Map<String,Value>,field:&str) -> Option<u32> { meta.get(field).and_then(Value::as_u64).and_then(|p|u32::try_from(p).ok()).filter(|p|*p != 0) }

/// Fora: sem terminal, em preparação, sem app-server (`app_pid` nulo é a troca de modo em curso) e
/// transferência que ainda não terminou (a regra do `sessions.list_all`).
pub(super) fn candidate(meta:&Map<String,Value>,home:&Path) -> Option<Candidate> {
    if meta.get("headless").is_some_and(truthy) || meta.get("launching").is_some_and(truthy) { return None; }
    let (name,endpoint,thread_id) = (text(meta,"name")?.to_owned(),text(meta,"endpoint")?.to_owned(),text(meta,"thread_id")?.to_owned());
    let app_pid = pid(meta,"app_pid")?;
    if let Some(transfer) = text(meta,"transfer_id") && !transfer_complete(home,transfer) { return None; }
    Some(Candidate { name,meta:meta.clone(),endpoint,app_pid,thread_id,key:text(meta,"key").map(str::to_owned),tui_pid:pid(meta,"tui_pid") })
}

// ponytail: só o `transfer_id` do sidecar; o índice por thread do Python (`transfer_for_thread`) fica de
// fora. Sessão de destino sem `transfer_id` é adotada mesmo com a transferência em curso.
pub(super) fn transfer_complete(home:&Path,id:&str) -> bool { transfer_state(home,id).unwrap_or(false) }

/// A transferência `id` terminou? O registro ausente ou ilegível é erro: o sidecar aponta para ele.
pub(super) fn transfer_state(home:&Path,id:&str) -> Result<bool,RuntimeError> {
    let id = id.to_ascii_lowercase();
    let unreadable = ||RuntimeError::new("codex_transfer_record_unreadable","o registro da transferência desta sessão não pôde ser lido");
    if id.len() != 36 || !id.bytes().all(|b|b.is_ascii_hexdigit() || b == b'-') { return Err(unreadable()); }
    let record = home.join(".hangar").join("conversation-transfers").join(format!("{id}.json"));
    let raw = match std::fs::read(record) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
            return Err(RuntimeError::new("codex_transfer_record_missing","o sidecar aponta para uma transferência cujo registro sumiu")),
        Err(_) => return Err(unreadable()),
    };
    serde_json::from_slice::<Value>(&raw).map(|record|record["phase"] == "complete").map_err(|_|unreadable())
}

/// O pane da sessão: o do Codex, e entre eles o ativo.
pub(super) fn pane_of<'a>(panes:&'a [Pane],name:&str) -> Option<&'a Pane> {
    panes.iter().filter(|p|p.session == name).max_by_key(|p|(p.provider.as_deref() == Some("codex"),p.active))
}

/// Quanto a conversa pode ficar abrindo (sidecar `launching` ou pane sem sidecar) antes de virar erro.
const LAUNCH_WAIT:Duration = Duration::from_secs(120);

pub(super) fn queue_files(queue_dir:&Path,key:&str) -> (PathBuf,PathBuf) {
    let runtime = queue_dir.join("runtime");
    (runtime.join(format!("{key}.json")),runtime.join(format!("{key}.lock")))
}

/// A chave vira nome de arquivo da fila.
pub(super) fn valid_key(key:&str) -> bool { !key.is_empty() && key.len() <= 128 && key.bytes().all(|b|b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

/// Grava a vida da adoção na geração `generation` (atual = só fatos, +1 = vida nova). Vida e nome já
/// gravados na mesma geração não escrevem nada: a readoção a cada reinício do Rust não gera recibo.
/// Devolve se escreveu.
pub(super) fn advance_life(store:&mut Store,generation:u64,name:&str,life:Value,clock:ClockSample) -> std::io::Result<bool> {
    let current = store.state().generation;
    if generation == current && store.state().name == name && store.state().runtime_state[CODEX_LIFE] == life { return Ok(false); }
    let state = json!({"codex_life":life,"name":name});
    let id = life_call_id(store.state(),generation,&state);
    store.exec(current,&id,clock,Action::AdvanceLifecycle { generation,state })?;
    Ok(true)
}

/// Identificador do `AdvanceLifecycle`. O `exec` devolve recibo de mesmo id e mesmo conteúdo sem aplicar,
/// então o id carrega o ponto de partida inteiro: nome, vida e o `next_seq` da fila, que sobe a cada
/// escrita aplicada (`queue.rs`, `exec`). Com ele, A→B→A→B nunca reencontra o recibo do primeiro B. A
/// repetição sem escrita no meio não chega aqui (`advance_life`). Chaves ordenadas: o `serde_json` daqui
/// preserva a ordem de inserção.
pub(super) fn life_call_id(from:&QueueState,generation:u64,state:&Value) -> String {
    let origin = json!({"name":from.name,"codex_life":from.runtime_state[CODEX_LIFE],"next_seq":from.next_seq});
    let state = &json!({"from":origin,"to":state});
    fn canonical(value:&Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys:Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|key|(key.clone(),canonical(&map[key]))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    let digest = ring::digest::digest(&ring::digest::SHA256,canonical(state).to_string().as_bytes());
    let short:String = digest.as_ref()[..8].iter().map(|b|format!("{b:02x}")).collect();
    format!("codex-life:{generation}:{short}")
}

fn life_marker(created:u64,pane:&str,tui_pid:Option<u32>,endpoint:&str,app_pid:u32) -> Value {
    json!({"created":created,"pane":pane,"tui_pid":tui_pid,"endpoint":endpoint,"app_pid":app_pid})
}

/// Mesma vida do terminal: o nascimento da sessão (o escritor do pane confere o mesmo, inclusive no psmux)
/// e, fora do psmux (onde o pane é sempre `%1`), o id do pane. O pid da TUI só conta quando os dois lados
/// o têm: o sidecar quase nunca o guarda, e ausente é "ainda não conhecido", não outra TUI.
fn same_pane_life(a:&Value,b:&Value) -> bool {
    let both = |field:&str|!a[field].is_null() && !b[field].is_null();
    a["created"] == b["created"]
        && (cfg!(windows) || !both("pane") || a["pane"] == b["pane"])
        && (!both("tui_pid") || a["tui_pid"] == b["tui_pid"])
}

fn same_server(a:&Value,b:&Value) -> bool { a["endpoint"] == b["endpoint"] && a["app_pid"] == b["app_pid"] }

pub(super) fn io_failure(error:std::io::Error) -> RuntimeError {
    if error.kind() == std::io::ErrorKind::WouldBlock { return RuntimeError::new("runtime_lease","trava da fila presa por outro dono"); }
    RuntimeError::new("queue_io",&format!("fila da adoção inacessível: {:?}",error.kind()))
}

/// A conversa foi adotada entre a decisão e a gravação: a fila agora é do ator, e o texto vai por ele.
pub const ADOPTED_MEANWHILE:&str = "codex_terminal_adopted_meanwhile";

fn launch_stuck() -> RuntimeError {
    RuntimeError::new("codex_terminal_launch_stuck","o terminal do Codex está abrindo há tempo demais; feche e abra a sessão de novo")
}

/// Texto para a conversa que ainda abre, na fila da chave dela, como o kick-off (`pre_transcript`): a
/// adoção (`prepare`) mantém as linhas e o ator as entrega pelo app-server. Bloqueante.
fn queue_before_adoption(queue_dir:&Path,key:&str,name:&str,text:&str) -> Result<(),RuntimeError> {
    let (state_path,lease_path) = queue_files(queue_dir,key);
    if let Some(dir) = state_path.parent() { std::fs::create_dir_all(dir).map_err(io_failure)?; }
    let _lease = lease_blocking(&lease_path).map_err(io_failure)?;
    let mut store = Store::open(&state_path,queue_dir,QueueState::new(key,1,name,Vec::new())).map_err(io_failure)?;
    // Escrever ao lado do ator duplicaria o dono da fila.
    if !store.state().runtime_state[CODEX_LIFE].is_null() {
        return Err(RuntimeError::new(ADOPTED_MEANWHILE,"a fila desta sessão já é de uma conversa aberta; feche e abra a sessão se o erro continuar"));
    }
    let epoch_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64());
    let id = crate::mods::state::random_hex(16);
    let generation = store.state().generation;
    store.exec(generation,&format!("append:{id}"),ClockSample { monotonic_s:0.0,epoch_s },
        Action::Append { text:text.into(),delivered:false,ts:Some(epoch_s),pre_transcript:true,entry_id:Some(id) }).map_err(io_failure)?;
    Ok(())
}

/// A trava solta pelo ator que acabou de parar pode demorar um pouco (as tarefas de E/S dele).
pub(super) fn lease_blocking(path:&Path) -> std::io::Result<Arc<super::queue::Lease>> {
    super::queue::acquire_lease_within(path,Duration::from_secs(3))
}

/// Chave e geração da adoção, com a vida gravada na fila. O mesmo terminal e o mesmo app-server mantêm a
/// geração (o Rust que reiniciou readota); outro app-server do mesmo terminal sobe a geração; outro
/// terminal com a mesma chave (o `sessions.save` conserva a chave entre vidas de mesmo nome) gira a chave,
/// e a fila da sessão anterior não é entregue nesta. O pid da TUI que faltava entra na vida gravada.
fn prepare(sidecar_dir:&Path,queue_dir:&Path,c:&Candidate,life:&Value) -> Result<(String,u64),RuntimeError> {
    let changed = ||RuntimeError::new("codex_sidecar_changed","o sidecar mudou de vida durante a adoção");
    let mut expected = c.key.clone();
    // Mensagem gravada só na projeção pelo nome (o Python, com o Rust fora) antes da adoção: abrir a fila
    // regrava a projeção e a apagaria calada. Lida antes.
    let projected = pending_projection(queue_dir,&c.name)?;
    let mut foreign:Option<Vec<Value>> = None;
    for _ in 0..2 {
        let key = match &expected {
            Some(key) => key.clone(),
            None => codex_sidecar::swap_key(sidecar_dir,&c.name,&c.endpoint,c.app_pid,None,&crate::mods::state::random_hex(16))
                .map_err(io_failure)?.ok_or_else(changed)?,
        };
        if !valid_key(&key) { return Err(RuntimeError::new("codex_key","chave do sidecar inválida para a fila")); }
        let (state_path,lease_path) = queue_files(queue_dir,&key);
        let lease = acquire_lease(&lease_path).map_err(io_failure)?;
        let existed = state_path.exists();
        let mut store = Store::open(&state_path,queue_dir,QueueState::new(&key,1,&c.name,Vec::new())).map_err(io_failure)?;
        // Só o que a fila não tem: linhas dela na projeção são dela (inclusive as de uma vida que sai).
        let foreign = foreign.get_or_insert_with(||projected.iter().filter(|row|!store.state().rows.iter().any(|own|own["id"] == row["id"])).cloned().collect());
        let recorded = store.state().runtime_state[CODEX_LIFE].clone();
        let current = store.state().generation;
        let mut life = life.clone();
        if life["tui_pid"].is_null() && !recorded.is_null() && same_pane_life(&recorded,&life) { life["tui_pid"] = recorded["tui_pid"].clone(); }
        let generation = if !existed { 1 }
            else if recorded.is_null() { current + 1 }
            else if !same_pane_life(&recorded,&life) {
                super::queue::discard(&state_path,queue_dir,false).map_err(io_failure)?;
                drop(store);
                drop(lease);
                let fresh = crate::mods::state::random_hex(16);
                expected = Some(codex_sidecar::swap_key(sidecar_dir,&c.name,&c.endpoint,c.app_pid,Some(&key),&fresh).map_err(io_failure)?.ok_or_else(changed)?);
                continue;
            }
            else if same_server(&recorded,&life) { current } else { current + 1 };
        // Escritor único da geração: o mesmo `AdvanceLifecycle` da troca de modo (atual = só fatos, +1 = vida nova).
        let clock = ClockSample { monotonic_s:0.0,epoch_s:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0) };
        advance_life(&mut store,generation,&c.name,life,clock).map_err(io_failure)?;
        for row in foreign.iter() {
            let id = row["id"].as_str().unwrap_or_default();
            if store.state().rows.iter().any(|own|own["id"] == id) { continue; }
            store.exec(store.state().generation,&format!("adopt-pending:{id}"),clock,Action::Append { text:row["text"].as_str().unwrap_or_default().into(),
                delivered:false,ts:row["ts"].as_f64(),pre_transcript:row["pre_transcript"] == true,entry_id:Some(id.into()) }).map_err(io_failure)?;
        }
        return Ok((key,generation));
    }
    Err(changed())
}

/// Entradas não entregues da projeção pelo nome: as que o Python gravou sem a fila desta chave. Ausente é
/// fila vazia; ilegível recusa a adoção, que regravaria a projeção por cima do que não leu.
fn pending_projection(queue_dir:&Path,name:&str) -> Result<Vec<Value>,RuntimeError> {
    let raw = match super::queue::read_replaced_to_string(&super::queue::projection_path(queue_dir,name)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(RuntimeError::new("codex_queue_projection_unreadable",
            &format!("a fila da sessão não pôde ser lida ({:?}); adotar a apagaria",error.kind()))),
    };
    let mut skipped = 0usize;
    let rows = raw.lines().filter(|line|!line.trim().is_empty()).filter_map(|line|match serde_json::from_str::<Value>(line) {
        Ok(row) => Some(row),
        Err(_) => { skipped += 1; None }
    }).collect::<Vec<_>>();
    if skipped > 0 {
        tracing::warn!(session=%name,code="codex_queue_projection_line_unreadable",skipped,"linhas ilegíveis na fila da sessão ficaram de fora da adoção");
    }
    Ok(rows.into_iter().filter(|row|row["delivered"] == false && row["papel"].is_null() && row["text"].is_string()
        && row["id"].as_str().is_some_and(|id|!id.is_empty())).collect())
}

impl Watcher {
    pub fn new(cfg:WatchConfig) -> Self {
        Self { cfg,misses:Default::default(),death_after:DEATH_AFTER,legacy_failures:Default::default(),scope_probe:crate::session_open::terminal::systemd_scope }
    }
    pub fn with_scope_probe(mut self,probe:fn()->Vec<String>) -> Self { self.scope_probe = probe; self }
    /// O multiplexador que cria panes, com o escopo desta vigia.
    pub(super) fn spawn_mux(&self) -> crate::session_open::terminal::ProcessMux {
        crate::session_open::terminal::ProcessMux { probe:self.scope_probe,..Default::default() }
    }
    pub(super) fn mux_program(&self) -> String { self.cfg.mux_program.to_string_lossy().into_owned() }
    /// Prazo de ausência antes da morte; os testes encurtam.
    pub fn with_death_after(mut self,after:Duration) -> Self { self.death_after = after; self }

    /// A ausência de `id` dura o prazo? A primeira vez só marca o início.
    fn absent_long_enough(&self,misses:&mut BTreeMap<String,std::time::Instant>,id:&str) -> bool {
        let since = *misses.entry(id.to_owned()).or_insert_with(std::time::Instant::now);
        if since.elapsed() < self.death_after { return false; }
        misses.remove(id);
        true
    }

    /// Uma rodada por segundo, ou na hora em que um ator adotado perde o app-server.
    pub async fn run(self:Arc<Self>,registry:Arc<RuntimeRegistry>) {
        let wake = registry.codex_wake();
        loop {
            self.round(&registry).await;
            tokio::select! {
                () = tokio::time::sleep(WATCH_EVERY) => {},
                () = wake.notified() => {},
            }
        }
    }

    /// Lê os sidecars e os panes e acerta o registro: vida trocada fecha, pane morto encerra, sessão nova
    /// é adotada. Sem resposta do multiplexador nada se decide: sessão que não se vê não é sessão morta.
    pub async fn round(&self,registry:&RuntimeRegistry) {
        let mut misses = self.misses.lock().await;
        let dir = codex_sidecar::dir(&self.cfg.home);
        let sidecars = tokio::task::spawn_blocking(move || {
            let mut problems = crate::list::discover::Problems::default();
            crate::list::discover_other::sidecars(&dir,"sidecar Codex ilegível ou torto",&mut problems)
        }).await.unwrap_or_default();
        let panes = match self.cfg.mux.list_panes().await {
            Ok(panes) => panes,
            Err(error) => {
                if crate::warn_limit::allow(None,"codex_watch_panes_unavailable") {
                    tracing::warn!(code="codex_watch_panes_unavailable",reason=%error.code,"vigia do Codex com terminal sem a lista de panes; rodada sem decisão");
                }
                return;
            }
        };
        let home = &self.cfg.home;
        let mut seen = BTreeSet::new();
        for target in registry.codex_terminals().await {
            let raw = sidecars.iter().map(|(_,meta)|meta).find(|meta|text(meta,"key") == Some(target.key.as_str()));
            match raw.map(|meta|candidate(meta,home)) {
                // Renomeada, app-server ressubido ou troca de modo em curso: esta vida acaba sem matar nada,
                // e a adoção abaixo pega a nova quando ela estiver pronta.
                Some(Some(c)) if c.name != target.name || c.endpoint != target.endpoint || c.app_pid != target.app_pid => self.release(registry,&target).await,
                Some(None) => self.release(registry,&target).await,
                _ => {
                    // O pid da TUI relido a cada rodada: o lançador costuma gravá-lo depois da adoção.
                    let tui = raw.and_then(|meta|pid(meta,"tui_pid")).or_else(||pid(target.metadata.as_object()?,"tui_pid"));
                    if !gone(&panes,&target.name,Some((target.terminal.pane.as_str(),target.terminal.created)),tui).await { continue; }
                    seen.insert(target.key.clone());
                    if self.absent_long_enough(&mut misses,&target.key) { self.bury(registry,&target).await; }
                }
            }
        }
        for meta in sidecars.iter().map(|(_,meta)|meta) {
            if let (Some(name),Some(transfer)) = (text(meta,"name"),text(meta,"transfer_id"))
                && let Err(error) = transfer_state(home,transfer) && crate::warn_limit::allow(Some(name),&error.code) {
                tracing::warn!(session=%name,code=%error.code,reason=%error.message,"Codex com terminal não adotado");
            }
            let Some(c) = candidate(meta,home) else { continue };
            // Entrada com terminal de outra vida (o Claude que saiu deste nome) não reserva o nome para o Codex.
            if let Err(error) = self.retire_other_life(registry,&c.name).await && crate::warn_limit::allow(Some(&c.name),&error.code) {
                tracing::warn!(session=%c.name,code=%error.code,"entrada de outra vida não saiu; o Codex espera");
                registry.report("runtime.stale_life_failed",&c.name,&error.code,"a entrada de outra vida não saiu e o Codex não foi adotado");
            }
            if registry.has_entry(c.key.as_deref(),&c.name).await { continue; }
            match pane_of(&panes,&c.name) {
                Some(pane) => if let Err(error) = self.adopt(registry,c.clone(),pane).await
                    && crate::warn_limit::allow(Some(&c.name),&error.code) {
                    tracing::warn!(session=%c.name,code=%error.code,reason=%error.message,"Codex com terminal não adotado");
                },
                // Sem ator e sem pane: a sessão morreu com o Rust fora; a vigia limpa o que sobrou.
                None if gone(&panes,&c.name,None,c.tui_pid).await => {
                    let orphan = format!("\0{}\0{}\0{}",c.name,c.endpoint,c.app_pid);
                    seen.insert(orphan.clone());
                    if self.absent_long_enough(&mut misses,&orphan) {
                        let projection = !registry.has_entry(None,&c.name).await;
                        self.clean(&c.name,&c.endpoint,c.app_pid,c.key.clone(),projection).await;
                    }
                }
                None => {}
            }
        }
        misses.retain(|id,_|seen.contains(id));
    }

    /// Escrita sem ator aberto: adota agora o Codex com terminal de `name`, ou abre o sem terminal parado
    /// (`open_parked`). `None` = não é Codex do Rust, e a escrita segue o caminho de sempre.
    pub async fn adopt_now(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<(),RuntimeError>> {
        // Subir o sem terminal pode levar segundos: fora da exclusão da vigia.
        let adopted = { let _round = self.misses.lock().await; self.adopt_locked(registry,name).await };
        match adopted {
            None => match self.open_parked(registry,name).await {
                // A conversa ainda abre: é do Rust, e quem não é texto espera por ela.
                None => self.opening(name).await.map(|opening|opening.and(Err(super::codex_relaunch::starting()))),
                parked => parked,
            },
            Some(Err(error)) if error.code == LEGACY_SIDECAR => self.relaunch_legacy(registry,name).await,
            adopted => adopted,
        }
    }

    /// O Codex com terminal que ainda abre a conversa: pane Codex sem sidecar (o lançador o grava no
    /// `thread/started`) ou sidecar `launching`. `Some(Ok)` = a chave da fila dessa vida (a `CP_SESSION_KEY`
    /// do pane, que o lançador copia no sidecar); `None` = não está abrindo.
    pub async fn opening(&self,name:&str) -> Option<Result<String,RuntimeError>> {
        let no_key = ||RuntimeError::new("codex_terminal_not_ready","o terminal do Codex abriu sem a chave da sessão");
        let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),name);
        let read = tokio::task::spawn_blocking(move||codex_sidecar::read(&file)).await
            .unwrap_or_else(|error|Err(std::io::Error::other(error.to_string())));
        // Sidecar que existe e não se lê não prova que a conversa ainda abre: erro, nunca a chave do pane.
        let meta = match read {
            Ok(meta) => meta,
            Err(error) => return Some(Err(RuntimeError::new("codex_sidecar_unreadable",&format!("o arquivo da sessão não pôde ser lido ({:?})",error.kind())))),
        };
        if let Some(meta) = meta {
            if meta.get("headless").is_some_and(truthy) || !meta.get("launching").is_some_and(truthy) { return None; }
            return Some(text(&meta,"key").filter(|key|valid_key(key)).map(str::to_owned).ok_or_else(no_key));
        }
        let panes = self.cfg.mux.list_panes().await.ok()?;
        let pane = pane_of(&panes,name).filter(|pane|pane.provider.as_deref() == Some("codex"))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|d|d.as_secs());
        if pane.session_created.is_some_and(|born|now.saturating_sub(born) > LAUNCH_WAIT.as_secs()) { return Some(Err(launch_stuck())); }
        let Some(shown) = self.mux_text(&["show-environment","-t",&super::claude_terminal::session_target(name),"CP_SESSION_KEY"]).await else {
            return Some(Err(RuntimeError::new("codex_terminal_mux","o multiplexador não disse a chave da sessão")));
        };
        Some(super::claude_terminal::session_key(&shown).ok_or_else(no_key))
    }

    /// Guarda o texto na fila da conversa que ainda abre (`key` vem de `opening`).
    pub async fn queue_opening(&self,name:&str,key:&str,text:&str) -> Result<(),RuntimeError> {
        let (queue_dir,key,name,text) = (self.cfg.queue_dir.clone(),key.to_owned(),name.to_owned(),text.to_owned());
        tokio::task::spawn_blocking(move||queue_before_adoption(&queue_dir,&key,&name,&text)).await
            .unwrap_or_else(|_|Err(RuntimeError::new("queue_io","a gravação da fila foi interrompida")))
    }

    /// `adopt_now` para quem já segura a exclusão da vigia (a troca de modo).
    pub(super) async fn adopt_locked(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<(),RuntimeError>> {
        let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),name);
        let meta = tokio::task::spawn_blocking(move||codex_sidecar::load(&file)).await.ok().flatten()?;
        if meta.get("headless").is_some_and(truthy) { return None; }
        if let Err(error) = self.retire_other_life(registry,name).await { return Some(Err(error)); }
        if registry.has_entry(None,name).await { return Some(Ok(())); }
        // TUI ainda subindo: o texto espera na fila da chave (`opening`) e a adoção o assume. Presa
        // abrindo além do prazo, é erro com código.
        if meta.get("launching").is_some_and(truthy) {
            let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),name);
            let age = tokio::task::spawn_blocking(move||std::fs::metadata(file).and_then(|m|m.modified()).ok()
                .and_then(|at|at.elapsed().ok())).await.ok().flatten();
            if age.is_none_or(|age|age < LAUNCH_WAIT) { return None; }
            return Some(Err(launch_stuck()));
        }
        if text(&meta,"endpoint").is_none() {
            return Some(Err(RuntimeError::new(LEGACY_SIDECAR,"o arquivo da sessão é do desenho antigo, sem o endereço do app-server")));
        }
        if let Some(transfer) = text(&meta,"transfer_id") && let Err(error) = transfer_state(&self.cfg.home,transfer) { return Some(Err(error)); }
        let Some(c) = candidate(&meta,&self.cfg.home) else {
            return Some(Err(RuntimeError::new("codex_terminal_not_ready","o Codex com terminal ainda não tem app-server pronto")));
        };
        let panes = match self.cfg.mux.list_panes().await {
            Ok(panes) => panes,
            Err(error) => return Some(Err(RuntimeError::new("codex_terminal_mux",&format!("o multiplexador não respondeu ({})",error.code)))),
        };
        let Some(pane) = pane_of(&panes,&c.name) else {
            return Some(Err(RuntimeError::new("codex_terminal_absent","o terminal da sessão não está aberto")));
        };
        Some(self.adopt(registry,c,pane).await)
    }

    /// Encerrar pelo app: o pane primeiro (sobrevivendo, nada mais muda, como no `registry.kill` do Python),
    /// depois a vida inteira como na morte do pane, mais o shell `term-<nome>` que o app abriu.
    pub async fn kill(&self,registry:&RuntimeRegistry,target:&CodexTerminalTarget) -> Result<Value,RuntimeError> {
        let _round = self.misses.lock().await;
        self.confirm_life(target).await?;
        self.kill_pane(&target.name).await?;
        registry.close(&target.key,target.generation).await?;
        let projection = !registry.has_entry(None,&target.name).await;
        self.clean(&target.name,&target.endpoint,target.app_pid,Some(target.key.clone()),projection).await;
        Ok(json!({"closed":true,"killed":true}))
    }

    /// `DELETE` que entrou pelo Python. `None` = não é Codex com terminal (o Python encerra como hoje). Sem
    /// ator aberto (vida ainda não pronta), o pane e o que a vida comprovar; o resto é das lojas do Python.
    /// O `Ok` leva `transcript` (a conversa atual do sidecar) quando ela é conhecida: o sidecar some no
    /// encerramento, e o Python precisa dela para guardar o nome fechado.
    pub async fn kill_by_name(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<Value,RuntimeError>> {
        let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),name);
        // Só a ausência devolve ao Python: sidecar que existe e não se lê não prova que a sessão não é do
        // Rust, e o Python encerraria sem a marca dele.
        let read = match tokio::task::spawn_blocking(move||codex_sidecar::read(&file)).await {
            Ok(Ok(meta)) => Ok(meta),
            Ok(Err(error)) => {
                tracing::warn!(session = %name, code = "close_sidecar_unreadable", kind = ?error.kind(), "sidecar do Codex ilegível ao encerrar");
                Err(RuntimeError::new("close_sidecar_unreadable","o arquivo da sessão Codex não pôde ser lido; nada foi encerrado"))
            }
            Err(_) => Err(RuntimeError::new("close_sidecar_unreadable","a leitura do arquivo da sessão Codex falhou; nada foi encerrado")),
        };
        let meta = read.as_ref().ok().cloned().flatten();
        let transcript = meta.as_ref().and_then(|m|text(m,"rollout_path")).filter(|p|!p.is_empty()).map(str::to_owned);
        let with_transcript = |mut value:Value,fallback:Option<&Path>| {
            if let Some(path) = transcript.clone().or_else(||fallback.map(|p|p.to_string_lossy().into_owned())) { value["transcript"] = json!(path); }
            value
        };
        if let Some(target) = registry.codex_terminals().await.into_iter().find(|t|t.name == name) {
            return Some(self.kill(registry,&target).await.map(|v|with_transcript(v,target.transcript.as_deref())));
        }
        let meta = match read { Ok(meta) => meta?, Err(error) => return Some(Err(error)) };
        if meta.get("headless").is_some_and(truthy) { return None; }
        let _round = self.misses.lock().await;
        if let Err(error) = self.kill_pane(name).await { return Some(Err(error)); }
        if let Some(c) = candidate(&meta,&self.cfg.home) {
            let projection = !registry.has_entry(None,name).await;
            self.clean(name,&c.endpoint,c.app_pid,c.key,projection).await;
        }
        Some(Ok(with_transcript(json!({"closed":true,"killed":true}),None)))
    }

    /// Encerrar mira a vida, não o nome: logo depois de um renomear (ou com outra sessão no nome antigo) o
    /// `kill-session` pelo nome derrubaria outro pane e o SIGTERM iria no app-server de uma sessão viva.
    /// Exige o sidecar do nome com a chave, o endereço e o pid desta vida, e o pane do nome com o mesmo
    /// nascimento (e id, fora do psmux); sem pane no nome, o desta vida não pode estar vivo sob outro.
    // ponytail: um renomear entre esta conferência e o `kill-session` (milissegundos) ainda escapa.
    pub(super) async fn confirm_life(&self,target:&CodexTerminalTarget) -> Result<(),RuntimeError> {
        let stale = ||RuntimeError::new("close_kill_stale","a sessão com este nome não é mais a que está aberta aqui; nada foi encerrado");
        let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),&target.name);
        let meta = tokio::task::spawn_blocking(move||codex_sidecar::load(&file)).await.ok().flatten().ok_or_else(stale)?;
        if text(&meta,"key") != Some(target.key.as_str()) || text(&meta,"endpoint") != Some(target.endpoint.as_str())
            || pid(&meta,"app_pid") != Some(target.app_pid) { return Err(stale()); }
        let panes = self.cfg.mux.list_panes().await
            .map_err(|_|RuntimeError::new("close_kill_mux","o multiplexador não respondeu; nada foi encerrado"))?;
        let born = |p:&Pane|p.session_created == Some(target.terminal.created);
        let ours = |p:&Pane|born(p) && (cfg!(windows) || p.pane_id == target.terminal.pane);
        let named:Vec<&Pane> = panes.iter().filter(|p|p.session == target.name).collect();
        if !named.is_empty() && !named.iter().any(|p|ours(p)) { return Err(stale()); }
        if named.is_empty() && panes.iter().any(|p|!cfg!(windows) && ours(p)) { return Err(stale()); }
        Ok(())
    }

    /// `kill-session` confirmado e o shell `term-<nome>` marcado; o pane sobrevivendo, nada mais muda.
    async fn kill_pane(&self,name:&str) -> Result<(),RuntimeError> {
        let mux = crate::state::capture::MuxProcess::new(&self.cfg.mux_program,crate::state::capture::TIMEOUT);
        match mux.kill_session(name).await {
            Some(true) => {},
            Some(false) => return Err(RuntimeError::new("close_kill_survived","o terminal da sessão não fechou; nada foi encerrado")),
            None => return Err(RuntimeError::new("close_kill_mux","o multiplexador não respondeu; nada foi encerrado")),
        }
        let hidden = format!("term-{name}");
        if mux.is_hidden(&hidden).await && mux.kill_session(&hidden).await != Some(true) {
            tracing::warn!(session=%name,"shell escondido do Codex com terminal não fechou");
        }
        Ok(())
    }

    /// A vida acabou sem a sessão morrer (renomear, app-server ressubido): só o ator sai.
    async fn release(&self,registry:&RuntimeRegistry,target:&CodexTerminalTarget) {
        if let Err(error) = registry.close(&target.key,target.generation).await {
            tracing::warn!(session=%target.name,code=%error.code,"vida antiga do Codex com terminal não fechou");
        }
    }

    /// O pane morreu: o ator sai, e app-server, sidecar e fila desta vida vão junto.
    async fn bury(&self,registry:&RuntimeRegistry,target:&CodexTerminalTarget) {
        if let Err(error) = registry.close(&target.key,target.generation).await {
            tracing::warn!(session=%target.name,code=%error.code,"Codex com terminal encerrado mas o ator não fechou; nada foi limpo");
            return;
        }
        let projection = !registry.has_entry(None,&target.name).await;
        self.clean(&target.name,&target.endpoint,target.app_pid,Some(target.key.clone()),projection).await;
        tracing::info!(session=%target.name,"Codex com terminal encerrou: ator, app-server, sidecar e fila limpos");
    }

    /// Mata só o app-server desta vida (o pid comprovado pelo endereço), apaga o sidecar só se ele ainda
    /// é dela e a fila da chave. `projection`: nenhuma outra vida usa o nome (a projeção é por nome).
    async fn clean(&self,name:&str,endpoint:&str,app_pid:u32,key:Option<String>,projection:bool) {
        let (dir,queue_dir) = (codex_sidecar::dir(&self.cfg.home),self.cfg.queue_dir.clone());
        let (name,endpoint) = (name.to_owned(),endpoint.to_owned());
        let job = tokio::task::spawn_blocking(move || {
            if app_server(app_pid,&endpoint) == AppServer::Ours && super::process::terminate(app_pid).is_err() {
                tracing::warn!(session=%name,pid=app_pid,"app-server do Codex com terminal não foi encerrado");
            }
            if let Err(error) = codex_sidecar::delete_if_life(&dir,&name,&endpoint,app_pid,key.as_deref()) {
                tracing::warn!(session=%name,kind=?error.kind(),"sidecar do Codex com terminal não foi apagado");
            }
            let Some(key) = key.filter(|key|valid_key(key)) else { return };
            let (state_path,lease_path) = queue_files(&queue_dir,&key);
            let discarded = lease_blocking(&lease_path).and_then(|_lease|super::queue::discard(&state_path,&queue_dir,projection));
            if let Err(error) = discarded { tracing::warn!(session=%name,kind=?error.kind(),"fila do Codex com terminal encerrado não foi apagada"); }
        });
        if job.await.is_err() { tracing::warn!("limpeza do Codex com terminal interrompida"); }
    }

    pub(super) async fn adopt(&self,registry:&RuntimeRegistry,c:Candidate,pane:&Pane) -> Result<(),RuntimeError> {
        let target_pane = pane.target().ok_or_else(||RuntimeError::new("codex_terminal_pane","pane do Codex sem endereço preciso"))?;
        // Sem nascimento não há identidade do terminal (o escritor do pane também a exige).
        let created = pane.session_created.ok_or_else(||RuntimeError::new("codex_terminal_pane","pane do Codex sem nascimento da sessão"))?;
        let life = life_marker(created,&pane.pane_id,c.tui_pid,&c.endpoint,c.app_pid);
        let (sidecar_dir,queue_dir) = (codex_sidecar::dir(&self.cfg.home),self.cfg.queue_dir.clone());
        let prepared = {
            let (c,sidecar_dir,queue_dir) = (c.clone(),sidecar_dir.clone(),queue_dir.clone());
            tokio::task::spawn_blocking(move||prepare(&sidecar_dir,&queue_dir,&c,&life)).await
                .map_err(|_|RuntimeError::new("codex_watch_job","preparo da adoção interrompido"))?
        };
        let (key,generation) = prepared?;
        let (state_path,lease_path) = queue_files(&queue_dir,&key);
        let mut metadata = Value::Object(c.meta.clone());
        metadata["key"] = json!(key);
        // O que está na fila antes do nascimento deste terminal é de outra vida (o `min_ts` do `Claim`).
        metadata["created"] = json!(created as f64);
        let terminal = crate::terminal_input::TerminalBinding { name:c.name.clone(),pane:target_pane,conversation:c.thread_id.clone(),generation,created,
            mux_argv:vec![self.cfg.mux_program.to_string_lossy().into_owned()],windows:cfg!(windows),clipboard_lock_path:None };
        let target = CodexTerminalTarget { key,generation,name:c.name,endpoint:c.endpoint,app_pid:c.app_pid,thread_id:c.thread_id,terminal,
            transcript:text(&c.meta,"rollout_path").map(PathBuf::from),metadata,lease_path,state_path,projection_dir:queue_dir,sidecar_dir:Some(sidecar_dir) };
        registry.open_codex_terminal(target).await.map(|_|())
    }
}

/// Fatos das rotas só de pane do Codex com terminal: o escritor do pane confere a identidade dele
/// (sessão, pane e nascimento); prontidão e pergunta são do app-server, não da tela.
struct PaneOnly { panels:Option<Arc<crate::term::Terms>> }

impl crate::terminal_input::TerminalServices for PaneOnly {
    /// Só o seletor de permissões pergunta: com o painel real aberto a janela tem o tamanho dele.
    fn permission_guard<'a>(&'a self,binding:&'a crate::terminal_input::TerminalBinding) -> crate::terminal_input::ServiceFuture<'a,()> {
        let open = self.panels.as_ref().is_some_and(|panels|panels.is_active(&binding.name));
        Box::pin(async move { if open { Err(crate::terminal_input::ServiceError("terminal_panel_open")) } else { Ok(()) } })
    }
    fn facts<'a>(&'a self,binding:&'a crate::terminal_input::TerminalBinding) -> crate::terminal_input::ServiceFuture<'a,crate::terminal_input::InputFacts> {
        let binding = binding.clone();
        Box::pin(async move { Ok(crate::terminal_input::InputFacts { binding,ready:true,idle:true,open_question:false,plugin_live:false,
            plugin_user:false,clipboard_available:false,native:None,screen:None }) })
    }
    fn publish<'a>(&'a self,_:&'a crate::terminal_input::TerminalBinding,_:crate::terminal_input::PluginRequest) -> crate::terminal_input::ServiceFuture<'a,crate::terminal_input::PluginReply> {
        Box::pin(async { Ok(crate::terminal_input::PluginReply::Unavailable) })
    }
}

/// Escritor do pane para `/keys`, `/term-input` e `/codex-permissions` do Codex com terminal: as mesmas
/// teclas permitidas e o mesmo texto literal do Claude com terminal, na trava da vida (`AdoptionCell::pane_serial`).
pub fn pane_driver(binding:&crate::terminal_input::TerminalBinding,serial:Arc<tokio::sync::Mutex<()>>,panels:Option<Arc<crate::term::Terms>>) -> crate::terminal_input::TerminalDriver {
    crate::terminal_input::TerminalDriver::new(binding.clone(),Arc::new(PaneOnly { panels }),Arc::new(crate::terminal_input::ProcessIo::default()),Default::default())
        .with_serial(serial)
}

/// A sessão acabou: o multiplexador respondeu sem o pane dela (nem pelo nome com o mesmo nascimento, nem,
/// fora do psmux, pelo id, que sobrevive ao renomear) e a TUI da vida morreu.
// ponytail: pid da TUI reaproveitado por outro processo segura a sessão como viva até o pid sair.
async fn gone(panes:&[Pane],name:&str,life:Option<(&str,u64)>,tui:Option<u32>) -> bool {
    let born = |pane:&Pane|life.is_none_or(|(_,c)|c == 0 || pane.session_created.is_none_or(|b|b == c));
    let by_id = |pane:&Pane|!cfg!(windows) && life.is_some_and(|(id,_)|pane.pane_id == id) && born(pane);
    if pane_of(panes,name).is_some_and(born) || panes.iter().any(by_id) { return false; }
    match tui {
        None => true,
        Some(pid) => tokio::task::spawn_blocking(move||super::process::argv(pid).is_none()).await.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_literal_loopback_address_is_accepted() {
        assert!(loopback_endpoint("ws://127.0.0.1:43210").is_ok());
        assert!(loopback_endpoint("ws://[::1]:43210").is_ok());
        for refused in ["ws://localhost:43210","ws://10.0.0.5:43210","ws://192.168.1.2:1","wss://127.0.0.1:43210",
            "ws://127.0.0.1:0","ws://127.0.0.1:43210/x","127.0.0.1:43210","ws://example.com:80","ws://127.0.0.1"] {
            assert_eq!(loopback_endpoint(refused).unwrap_err().code,"codex_endpoint","{refused}");
        }
    }

    #[test]
    fn reconnect_wait_grows_by_half_up_to_ten_seconds() {
        let mut reconnect = CanoReconnect::new(reconnect_waits());
        let mut waits = Vec::new();
        for _ in 0..9 { reconnect.schedule(0.0,false); waits.push(reconnect.next_at.unwrap()); }
        assert_eq!(&waits[..3],&[1.0,1.5,2.25]);
        assert_eq!(*waits.last().unwrap(),10.0);
        reconnect.connected();
        reconnect.schedule(5.0,false);
        assert_eq!(reconnect.next_at,Some(6.0),"conexão boa volta à primeira espera");
    }

    #[test]
    fn only_a_ready_terminal_sidecar_is_a_candidate() {
        let home = tempfile::tempdir().unwrap();
        let base = json!({"name":"cx","endpoint":"ws://127.0.0.1:1","app_pid":7,"thread_id":"t","tui_pid":9});
        let ok = |value:&Value|candidate(value.as_object().unwrap(),home.path());
        assert_eq!(ok(&base).map(|c|(c.app_pid,c.tui_pid,c.key)),Some((7,Some(9),None)));
        let transfer = "0000000A-0000-0000-0000-000000000001";
        for (field,value) in [("headless",json!(true)),("launching",json!(true)),("app_pid",Value::Null),("app_pid",json!(0)),
            ("endpoint",Value::Null),("thread_id",json!("")),("name",json!("")),("transfer_id",json!(transfer))] {
            let mut changed = base.clone();
            changed[field] = value;
            assert!(ok(&changed).is_none(),"{field}");
        }
        // Transferência concluída libera a adoção (o id vale em minúsculas, como o `uuid.UUID` do Python).
        let records = home.path().join(".hangar").join("conversation-transfers");
        std::fs::create_dir_all(&records).unwrap();
        std::fs::write(records.join(format!("{}.json",transfer.to_ascii_lowercase())),r#"{"phase":"complete"}"#).unwrap();
        let mut done = base.clone();
        done["transfer_id"] = json!(transfer);
        assert!(ok(&done).is_some());
    }

    #[test]
    fn the_conversation_is_not_part_of_the_life() {
        let terminal = crate::terminal_input::TerminalBinding { name:"cx".into(),pane:"%7".into(),conversation:"t1".into(),generation:1,created:5,
            mux_argv:vec!["tmux".into()],windows:false,clipboard_lock_path:None };
        let target = CodexTerminalTarget { key:"k".into(),generation:1,name:"cx".into(),endpoint:"ws://127.0.0.1:1".into(),app_pid:7,thread_id:"t1".into(),
            terminal,metadata:json!({}),transcript:None,lease_path:"l".into(),state_path:"s".into(),projection_dir:"p".into(),sidecar_dir:None };
        let mut switched = target.clone();
        switched.thread_id = "t2".into();
        switched.terminal.conversation = "t2".into();
        switched.transcript = Some("rollout-2".into());
        assert!(target.same_life(&switched),"a TUI trocou de conversa: mesma vida");
        let other = CodexTerminalTarget { app_pid:8,..target.clone() };
        assert!(!target.same_life(&other));
    }

    #[test]
    fn the_same_terminal_keeps_the_queue_and_another_rotates_it() {
        let life = life_marker(5,"%7",Some(9),"ws://127.0.0.1:1",7);
        let relaunched = life_marker(5,"%7",Some(9),"ws://127.0.0.1:2",8);
        assert!(same_pane_life(&life,&relaunched) && !same_server(&life,&relaunched));
        assert!(!same_pane_life(&life,&life_marker(5,"%7",Some(10),"ws://127.0.0.1:1",7)),"outra TUI conhecida dos dois lados");
        assert!(!same_pane_life(&life,&life_marker(6,"%7",Some(9),"ws://127.0.0.1:1",7)),"outro nascimento, também no psmux");
        assert_eq!(same_pane_life(&life,&life_marker(5,"%8",Some(9),"ws://127.0.0.1:1",7)),cfg!(windows),"no psmux o pane é sempre %1");
        // Pid da TUI ausente de um lado é "ainda não conhecido": não gira a chave.
        assert!(same_pane_life(&life_marker(5,"%7",None,"ws://127.0.0.1:1",7),&life));
        assert!(same_pane_life(&life,&life_marker(5,"%7",None,"ws://127.0.0.1:1",7)));
        // Sem pid dos dois lados, a vida nova de mesmo nome se separa pelo nascimento (o `null == null` do Windows).
        assert!(!same_pane_life(&life_marker(5,"%1",None,"ws://127.0.0.1:1",7),&life_marker(6,"%1",None,"ws://127.0.0.1:1",7)));
        // Vida gravada antes do id do pane entrar no marcador.
        assert!(same_pane_life(&json!({"created":5,"tui_pid":9,"endpoint":"ws://127.0.0.1:1","app_pid":7}),&life));
        assert!(valid_key("0123abcd_-") && !valid_key("") && !valid_key("../x") && !valid_key("a b"));
    }

    fn pane(session:&str,id:&str,created:u64) -> Pane {
        Pane { session:session.into(),pane_id:id.into(),provider:Some("codex".into()),session_created:Some(created),active:true,..Pane::default() }
    }

    #[tokio::test]
    async fn a_renamed_pane_is_alive_by_its_id_and_a_new_session_of_the_same_name_is_not() {
        let life = Some(("%7",5));
        assert!(!gone(&[pane("cx","%7",5)],"cx",life,None).await);
        // Renomeado e a rodada ainda não viu o sidecar novo: o id (fora do psmux) segura a vida.
        assert_eq!(gone(&[pane("cx-new","%7",5)],"cx",life,None).await,cfg!(windows));
        // Outra sessão com o mesmo nome (outro nascimento) não é esta vida.
        assert!(gone(&[pane("cx","%9",6)],"cx",life,None).await);
        assert!(gone(&[],"cx",life,None).await);
        // TUI viva segura a vida mesmo sem o pane.
        assert!(!gone(&[],"cx",life,Some(std::process::id())).await);
    }

    #[test]
    fn absence_counts_by_time_not_by_rounds() {
        let home = tempfile::tempdir().unwrap();
        let watcher = Watcher::new(WatchConfig { home:home.path().into(),queue_dir:home.path().into(),mux:Mux::default(),mux_program:"tmux".into() })
            .with_death_after(Duration::from_millis(200));
        let mut misses = BTreeMap::new();
        // Rodadas seguidas (o `wake`) não encerram antes do prazo.
        for _ in 0..5 { assert!(!watcher.absent_long_enough(&mut misses,"k")); }
        std::thread::sleep(Duration::from_millis(250));
        assert!(watcher.absent_long_enough(&mut misses,"k"));
        assert!(!misses.contains_key("k"),"encerrada, a marca sai");
    }

    #[test]
    fn this_test_process_is_not_an_app_server() {
        assert_eq!(app_server(std::process::id(),"ws://127.0.0.1:1"),AppServer::Foreign);
    }

    #[test]
    fn the_life_call_id_repeats_for_the_same_transition_only() {
        let life = json!({"created":1,"pane":"%7","tui_pid":null,"endpoint":"ws://127.0.0.1:1","app_pid":9});
        let from = QueueState::new("k",3,"cx",Vec::new());
        let id = life_call_id(&from,3,&json!({"codex_life":life,"name":"cx"}));
        assert!(id.starts_with("codex-life:3:"),"{id}");
        assert_eq!(id,life_call_id(&from,3,&json!({"name":"cx","codex_life":life})),"a ordem das chaves não muda o id: a readoção reusa o recibo");
        let mut tui = life.clone();
        tui["tui_pid"] = json!(42);
        let renamed_from = QueueState::new("k",3,"cx-renamed",Vec::new());
        let mut later = from.clone();
        later.next_seq += 1;
        assert_ne!(id,life_call_id(&later,3,&json!({"codex_life":life,"name":"cx"})),"depois de outra escrita da fila, o mesmo destino é outra operação");
        for other in [life_call_id(&from,4,&json!({"codex_life":life,"name":"cx"})),life_call_id(&from,3,&json!({"codex_life":life,"name":"cx-renamed"})),
            life_call_id(&from,3,&json!({"codex_life":tui,"name":"cx"})),life_call_id(&renamed_from,3,&json!({"codex_life":life,"name":"cx"}))] {
            assert_ne!(id,other,"outra geração, renomear, pid novo da TUI ou outro ponto de partida é outra operação");
        }
    }

    /// Renomear A→B→A→B na mesma geração, sobre a fila real e pelo `advance_life` da adoção: o `exec` devolve
    /// recibo repetido sem aplicar, então cada passo precisa de id próprio e a fila termina em B. A
    /// readoção repetida sem escrita no meio não grava nada.
    #[test]
    fn renaming_back_and_forth_in_the_same_generation_lands_on_the_last_name() {
        let dir = tempfile::tempdir().unwrap();
        let (state_path,lease_path) = queue_files(dir.path(),"k1");
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        let _lease = acquire_lease(&lease_path).unwrap();
        let mut store = Store::open(&state_path,dir.path(),QueueState::new("k1",1,"A",Vec::new())).unwrap();
        let life = json!({"created":1,"pane":"%7","tui_pid":null,"endpoint":"ws://127.0.0.1:1","app_pid":9});
        let clock = ClockSample { monotonic_s:0.0,epoch_s:1.0 };
        let step = |store:&mut Store,name:&str| advance_life(store,1,name,life.clone(),clock).unwrap();
        assert!(step(&mut store,"A"),"a primeira adoção grava a vida");
        let (seq,receipts) = (store.state().next_seq,store.state().operations.len());
        assert!(!step(&mut store,"A"),"readoção repetida: nada a gravar");
        assert_eq!((store.state().next_seq,store.state().operations.len()),(seq,receipts),"nenhum recibo novo a cada reinício do Rust");
        for name in ["B","A","B"] {
            assert!(step(&mut store,name),"{name}: renomear é escrita");
            assert_eq!(store.state().name,name,"aplicado, não devolvido de um recibo antigo de mesmo destino");
        }
        assert_eq!(store.state().name,"B");
    }
}
