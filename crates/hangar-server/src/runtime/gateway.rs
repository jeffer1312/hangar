use super::{actor::{LaunchConfig,PolicyClient,RuntimeActor,RuntimeEngine,RuntimeHandle},cano,process,protocol::*,queue::{Action,QueueActor,State as QueueState,Status,Store,acquire_lease_within}};
use super::codex_terminal::{Adoption,AdoptionCell,CodexTerminalTarget};
use axum::{Router,body::to_bytes,extract::{ConnectInfo,State,Request},http::StatusCode,
    middleware::{self,Next},response::{IntoResponse,Response,sse::{Event,KeepAlive,Sse}},routing::{get,post}};
use serde::Deserialize;
use serde_json::{Value,json};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration,SystemTime,UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast,Mutex};

/// `Headless`: ator sem terminal (cano). `Terminal`: entrada terminal do Claude. `CodexTerminal`: o mesmo
/// ator do Codex, sobre o WebSocket do app-server adotado, com a TUI no pane.
#[derive(Clone)]
pub(crate) enum EntryHandle {
    Headless(RuntimeHandle),
    Terminal {target:super::terminal::TerminalTarget,handle:super::terminal::TerminalHandle},
    CodexTerminal {target:Box<CodexTerminalTarget>,handle:RuntimeHandle,cell:AdoptionCell},
}
impl EntryHandle {
    pub(crate) async fn snapshot(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.snapshot().await,Self::Terminal {handle,..}=>handle.snapshot().await}}
    /// Só o ator do Codex/sem terminal tem motor próprio; o de terminal do Claude não é sessão Codex.
    pub(crate) async fn view(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.view().await,Self::Terminal {..}=>Err(failure("runtime_terminal"))}}
    pub(crate) async fn stop(&self)->Result<(),RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.stop().await,Self::Terminal {handle,..}=>handle.stop().await}}
    pub(crate) async fn command(&self,command:RuntimeCommand)->Result<RuntimeReply,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.command(command).await,Self::Terminal {handle,..}=>handle.command(command).await}}
    pub(crate) async fn queue(&self,id:String,action:Action)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.queue(id,action).await,Self::Terminal {handle,..}=>handle.queue(id,action).await}}
    pub(crate) async fn drain(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.drain().await,Self::Terminal {handle,..}=>handle.drain().await}}
    pub(crate) async fn confirm(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.confirm().await,Self::Terminal {handle,..}=>handle.confirm().await}}
    pub(crate) async fn ensure_projection(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h) | Self::CodexTerminal {handle:h,..}=>h.ensure_projection().await,Self::Terminal {handle,..}=>handle.ensure_projection().await}}
    /// Pane presente: entrada terminal do Claude ou Codex com TUI.
    pub(crate) fn has_terminal(&self) -> bool { !matches!(self,Self::Headless(_)) }
}
struct Entry { generation:u64,handle:EntryHandle,lease_path:std::path::PathBuf,name:String,mods_life:u64,provider:String,account_key:Option<crate::accounts::AccountKey>,relaunch:Option<Arc<super::claude_relaunch::Recipe>>,settings_owner:Option<Arc<super::claude_relaunch::Recipe>>,mode_source:crate::terminal_facts::ModeSource,live:LiveSender,runtime_target:Option<RuntimeTarget>,cano_birth:Option<f64>,auto_respawn:Option<Duration>,respawn_inflight:bool }

async fn runtime_account_guard(provider: &str, metadata: &Value) -> Result<crate::accounts::AccountGuard, RuntimeError> {
    use crate::accounts::{AccountKey, AccountLocks, Provider};
    let provider = if provider == "codex" { Provider::Codex } else { Provider::Claude };
    let field = if provider == Provider::Codex { "codex_home" } else { "config_dir" };
    let home = metadata[field].as_str().map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(|home| std::path::PathBuf::from(home).join(if provider == Provider::Codex { ".codex" } else { ".claude" }))
    }).ok_or_else(|| failure("account_home_unknown"))?;
    let key = tokio::task::spawn_blocking(move || AccountKey::new(provider, &home)).await
        .map_err(|_| failure("account_lock_worker"))?.map_err(|_| failure("account_key_invalid"))?;
    let locks = AccountLocks::system().map_err(|_| failure("account_locks_unavailable"))?;
    crate::accounts::claude_refresh::acquire_for_session(&locks, &key, std::time::Instant::now() + Duration::from_secs(5)).await
        .map_err(|_| failure("account_busy"))
}

/// Retrato do ator em até 1 s: `None` no prazo esgotado, `Some(false)` doente ou fora.
async fn probe(handle:&EntryHandle) -> Option<bool> {
    match tokio::time::timeout(Duration::from_secs(1),handle.snapshot()).await {
        Ok(Ok(view))=>Some(healthy(handle.has_terminal(),&view)),
        Ok(Err(_))=>Some(false),
        Err(_)=>None,
    }
}
/// Entrada aberta de uma sessão, achada por nome, para as rotas de escrita do Rust.
#[allow(dead_code)] // o `handle` é lido pelas rotas de escrita que entram depois
pub struct WriteTarget { pub key:String,pub generation:u64,pub provider:String,pub terminal:bool,pub healthy:bool,pub(crate) handle:EntryHandle }

/// Prazo do fechamento da porta: abaixo do `OP_TIMEOUT_S` (75 s) do transporte no Python.
const INGRESS_CLOSE_WAIT:Duration = Duration::from_secs(60);
pub struct RuntimeRegistry {
    interactions:Option<Arc<crate::interactions::InteractionStore>>,
    entries:Arc<Mutex<BTreeMap<String,Entry>>>,
    terminal_states:Option<Arc<crate::state::published::Published>>,
    terms:Option<Arc<crate::term::Terms>>,
    events:broadcast::Sender<RuntimeEvent>,
    policy:PolicyClient,
    instance:String,
    lifecycle:Mutex<BTreeMap<String,Arc<super::lifecycle::Barrier>>>,
    revisions:Mutex<BTreeMap<String,Arc<AtomicU64>>>,
    mods:Option<crate::mods::state::Mods>,
    plugin:Option<Arc<crate::plugin::Store>>,
    ingress:super::ingress::IngressGates,
    /// Primeira espera da religação do processo que cai (dobra a cada subida seguida).
    respawn_base:Duration,
    /// Esperas da religação ao mesmo cano que os atores do Claude sem terminal recebem.
    reconnect_waits:super::cano_reconnect::ReconnectWaits,
    permissions:Option<Arc<std::sync::Mutex<crate::state::permission::PermissionMemory>>>,
    processes:Arc<dyn crate::list::procs::ProcessView>,
    /// Último valor da sessão sem terminal por nome, para o feed do hub (`live`).
    live:std::sync::Mutex<BTreeMap<String,LiveSender>>,
    /// Nome → (chave, geração) do ator do Codex com terminal aberto aqui: o estado deles é do Rust.
    codex_owned:std::sync::Mutex<BTreeMap<String,(String,u64)>>,
    /// Hubs do chat, avisados quando essa posse muda.
    hubs:std::sync::OnceLock<crate::side::WeakHubs>,
    self_ref:std::sync::OnceLock<std::sync::Weak<RuntimeRegistry>>,
    /// Vigia do Codex com terminal; só existe quando a tabela entrega essas sessões ao Rust.
    watcher:std::sync::OnceLock<Arc<super::codex_terminal::Watcher>>,
    /// Acorda a vigia quando um ator adotado perde o app-server.
    codex_wake:Arc<tokio::sync::Notify>,
    pub(super) diag:crate::diag::DiagClient,
    /// Sessões com uma reabertura em erro em voo: o pedido seguinte da mesma sessão não abre outra.
    reopening:std::sync::Mutex<std::collections::BTreeSet<String>>,
    /// Problema do último retrato de cada ator com terminal (chave → geração, código), para a lista
    /// quando o `Monitor` não está vivo.
    terminal_problems:std::sync::Mutex<BTreeMap<String,(u64,String)>>,
    terminal_status:std::sync::Mutex<BTreeMap<String,super::terminal_status::StatusSender>>,
    /// Última reabertura por entrega sem prova, por chave: sobrevive à troca do ator que ela mesma faz.
    heal_attempts:std::sync::Mutex<BTreeMap<String,std::time::Instant>>,
    /// Caixa de entrada dos recibos dos recados nativos que o Rust escreve, com e sem terminal.
    native:super::native_message::Inbox,
    /// Quem troca de conta quando a sessão para no limite de uso (`session_write::limit_switch`).
    limits:std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<LimitHit>>,
    /// Sessões Claude paradas no limite à espera da troca: a fila não entrega nada até ela terminar.
    limit_holds:std::sync::Mutex<std::collections::BTreeSet<String>>,
}

/// O que o fim de turno achou: sem terminal quem vê o limite é o ator da sessão.
#[derive(Debug)]
pub enum TurnLimit { NotTerminal, Checked { transcript:std::path::PathBuf, limit:Option<super::protocol::UsageLimit>, sent:bool } }

/// Sessão que parou no limite de uso da conta.
#[derive(Debug)]
pub struct LimitHit {
    pub name:String, pub provider:String,
    /// A pasta de conta Claude da vida que bateu o limite (`None` = a padrão).
    pub account:Option<String>,
    pub limit:UsageLimit,
}
/// Intervalo mínimo entre duas reaberturas por entrega sem prova da mesma sessão: a que não resolveu
/// não vira laço de processos mortos.
pub(super) const HEAL_BACKOFF:Duration=Duration::from_secs(15*60);
#[derive(Clone,Copy,PartialEq,Eq)]
enum Reopen {Bypass,Heal}

/// Espera até 3 s pela trava; outro erro (pasta sem permissão, disco cheio) responde na hora.
async fn wait_lease(path:&std::path::Path) -> Result<Arc<super::queue::Lease>,RuntimeError> {
    let path = path.to_owned();
    match tokio::task::spawn_blocking(move ||acquire_lease_within(&path,Duration::from_secs(3))).await.map_err(|_|failure("queue_job"))? {
        Ok(lease)=>Ok(lease),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock=>Err(RuntimeError::new("runtime_lease","trava da sessão presa por outro dono há 3 s")),
        Err(error)=>Err(RuntimeError::new("runtime_lease",&format!("trava inacessível: {:?}",error.kind()))),
    }
}

/// Abre a fila sob a trava e roda `Recover`: entrada que estava em despacho quando o dono anterior
/// parou vira incerta antes de o ator olhar para ela.
async fn open_store(state_path:&std::path::Path,projection_dir:&std::path::Path,key:&str,generation:u64,name:&str,
    lease:Arc<super::queue::Lease>) -> Result<Store,RuntimeError> {
    let (state_path,projection_dir) = (state_path.to_owned(),projection_dir.to_owned());
    let initial = QueueState::new(key,generation,name,Vec::new());
    static OPENS:AtomicU64 = AtomicU64::new(0);
    let epoch_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0);
    let call_id = format!("open-recover:{}:{}:{}",std::process::id(),epoch_s,OPENS.fetch_add(1,std::sync::atomic::Ordering::Relaxed));
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let mut store = Store::open(&state_path,&projection_dir,initial)?;
        if store.state().generation != generation { return Ok(Err(failure("runtime_generation"))); }
        store.exec(generation,&call_id,ClockSample { monotonic_s:0.0,epoch_s },Action::Recover)?;
        store.ensure_projection()?;
        Ok(Ok(store))
    }).await
        .map_err(|_|failure("queue_job"))?.map_err(|error:std::io::Error|{
            // A frase da recusa da fila é fixa e diz por que a abertura falhou; o resto só pelo tipo.
            let reason = super::queue::refusal(&error).map_or_else(||format!("{:?}",error.kind()),str::to_owned);
            RuntimeError::new("queue_io",&format!("fila recusou: {reason}"))
        })?
}

/// Negação do `_rust_failed` do Python (erro vazio não é erro), exceto `terminal_facts`, que aqui é doente.
fn healthy(terminal:bool,view:&Value)->bool {
    let failed = match &view["error"] {Value::Null=>false,Value::String(code)=>!code.is_empty(),_=>true};
    if terminal { !failed || view["error"]=="receipt_scan" } else { !failed && view["view"]["alive"] != false }
}

fn failure(code:&str) -> RuntimeError { RuntimeError::new(code,"runtime indisponível para esta chave ou geração") }

/// Encerrar recusado porque a vida guardada não se comprova; ocupação e transferência não estão aqui.
/// Código interno da religação automática que achou a sessão fechada pelo nome; nunca sai do Rust.
const SESSION_CLOSED:&str="lifecycle_session_closed";
const LIFE_UNPROVEN:[&str;6]=["lifecycle_binding","lifecycle_binding_unavailable","lifecycle_process_birth_missing","lifecycle_process_changed","lifecycle_process_read","lifecycle_target_unavailable"];

/// `list-sessions` (`nome\tcriação`) ainda tem a sessão `name` desta vida. Outra criação no mesmo nome é
/// outra vida; criação ilegível não prova que é outra.
fn session_of_life(list:&str,name:&str,created:u64) -> bool {
    list.lines().filter_map(|line|line.split_once('\t')).filter(|(session,_)|*session==name)
        .any(|(_,born)|born.trim().parse::<u64>().map_or(true,|born|born==created))
}

/// Fila com mensagem ainda não entregue. Ilegível conta como pendente: abrir é o que mostra a recusa dela.
pub(super) fn pending(state_path:&std::path::Path) -> bool {
    match super::queue::read_replaced(state_path) {
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>false,
        Err(_)=>true,
        Ok(raw)=>serde_json::from_slice::<Value>(&raw).map_or(true,|state|state["rows"].as_array().is_some_and(|rows|rows.iter().any(|row|row["delivered"]!=true))),
    }
}

/// O cano gravado é desta vida (ou já saiu): só esse o Rust pode matar.
fn cano_is_ours(procs:&dyn crate::list::procs::ProcessView,target:&RuntimeTarget,born:f64,key:&str) -> Result<bool,RuntimeError> {
    super::lifecycle::process_absent(procs,i64::from(target.binding.pid),born).map(|absent|absent
        || crate::terminal_facts::agent_birth(target.binding.pid)==Some(born) && process::liveness(target.binding.pid,key)==process::Liveness::Ours)
}

/// `_OCIOSA_S` do Python: acima da janela de 1 h do cache do prompt, religar não custa cota a mais.
const PARK_AFTER:Duration = Duration::from_secs(65 * 60);
const PARK_ROUND:Duration = Duration::from_secs(60);

/// Regra 1 do módulo de processo: cano gravado `Ours` conecta (mesmo sem `versao`); morto, de outro
/// programa (pid reaproveitado) ou ausente, sobe outro com o ambiente calculado agora (nunca
/// guardado): o Codex por `codex_launch`, o Claude pelo spec da sessão (`claude_launch`).
pub(super) async fn launch_if_needed(policy:&PolicyClient,target:&mut RuntimeTarget,sidecar_dir:&std::path::Path) -> Result<Option<process::Cano>,RuntimeError> {
    let recorded = target.binding.pid;
    if recorded != 0 {
        let key = target.key.clone();
        let state = tokio::task::spawn_blocking(move||process::liveness(recorded,&key)).await.map_err(|_|failure("launch_job"))?;
        if state == process::Liveness::Ours { return Ok(None); }
    }
    let (base,dir,mut meta) = (policy.codex_environment(),sidecar_dir.to_owned(),target.metadata.clone());
    if let Value::Object(fields) = &mut meta { fields.insert("name".into(),json!(target.name)); fields.insert("key".into(),json!(target.key)); }
    let (claude,bridge,name,runtime) = (target.provider == "claude",policy.plugin_bridge(),target.name.clone(),tokio::runtime::Handle::current());
    let (program,env,stamp) = tokio::task::spawn_blocking(move||if claude {
        let launch = super::claude_launch::prepare(&base,bridge.as_ref(),&meta,&name,runtime)?;
        Ok((launch.program,launch.env,claude_config_stamp(meta["config_dir"].as_str().filter(|d|!d.is_empty()))))
    } else {
        super::codex_launch::launch_env(&base,&dir,&meta).map(|launch|(launch.program,launch.env,None))
    }).await.map_err(|_|failure("launch_job"))??;
    let cwd = target.metadata["cwd"].as_str().filter(|cwd|!cwd.is_empty()).ok_or_else(||failure("launch_cwd"))?;
    let spec = process::LaunchSpec { provider:process::Provider::from_str(&target.provider).ok_or_else(||failure("runtime_provider"))?,
        key:target.key.clone(),cwd:cwd.into(),program,env:env.into_iter().collect(),sidecar_dir:sidecar_dir.to_owned() };
    let cano = match spawn_unregistered(&spec,stamp).await {
        Ok(cano)=>cano,
        Err(error)=>{
            // O gravado já não serve (morto ou de outro programa): o arquivo da sessão para de apontá-lo.
            if recorded != 0 && matches!(error,process::ProcessError::NotListening) { clear_cano(target,sidecar_dir,recorded).await; }
            let detail = match &error { process::ProcessError::Spawn(detail)=>detail.as_str(),_=>"o processo da sessão não subiu" };
            return Err(RuntimeError::new(error.code(),detail));
        }
    };
    let value = serde_json::to_value(&cano).map_err(|_|failure("cano_json"))?;
    if let Err(error) = cano_recorded(launch_sidecar(target,sidecar_dir,"session.patch_meta",json!({"cano":value})).await) {
        // Sem o arquivo da sessão apontando para ele, o processo ficaria sem dono.
        if let Err(stop) = process::kill(&cano,&target.key,sidecar_dir).await {
            tracing::warn!(key=%target.key,code=stop.code(),"cano não gravado não foi encerrado");
        }
        return Err(error);
    }
    target.binding = CanoBinding { pid:cano.pid,escuta:cano.escuta.clone(),token:cano.token.clone(),versao:cano.versao };
    target.metadata["cano"] = value;
    Ok(Some(cano))
}
/// Marca da configuração que o `claude -p` leu ao nascer: o botão Recarregar e a adoção da reserva
/// pré-aquecida comparam com ela. Bloqueante.
pub(crate) fn claude_config_stamp(config_dir:Option<&str>) -> Option<String> {
    super::local_policy::config_stamp(config_dir)
        .inspect_err(|error|tracing::warn!(code=%error.code,"configuração da conta ilegível; sem marca de recarga")).ok()
}
/// Sobe o cano com o comando já montado, sem sessão registrada nem sidecar gravado: a subida da sessão
/// (`launch_if_needed`) e a reserva pré-aquecida. A marca vai no `cano` como `config_marca`.
pub(crate) async fn spawn_unregistered(spec:&process::LaunchSpec,stamp:Option<String>) -> Result<process::Cano,process::ProcessError> {
    let mut cano = process::spawn(spec).await?;
    if let Some(stamp) = stamp { cano.extra.insert("config_marca".into(),json!(stamp)); }
    Ok(cano)
}
/// A gravação do cano só vale confirmada: recusada como velha (`stale`), o processo novo ficaria sem o
/// arquivo da sessão apontar para ele.
fn cano_recorded(result:Result<Value,RuntimeError>) -> Result<(),RuntimeError> {
    if result?["updated"] == true { Ok(()) }
    else { Err(RuntimeError::new("session_patch_stale","o arquivo da sessão não aceitou o processo novo")) }
}
/// Arquivo da sessão na subida e na religação, gravado aqui (a vida é a chave, a vista vem do arquivo da
/// fila, como o Python lia).
pub(super) async fn launch_sidecar(target:&RuntimeTarget,sidecar_dir:&std::path::Path,kind:&str,payload:Value) -> Result<Value,RuntimeError> {
    let (dir,provider,name,key,state_path,kind) = (sidecar_dir.to_owned(),target.provider.clone(),target.name.clone(),target.key.clone(),target.state_path.clone(),kind.to_owned());
    tokio::task::spawn_blocking(move || {
        let saved = if kind == "session.clear_cano" { Value::Null } else { saved_view(&state_path)? };
        sidecar_service(&provider,&dir,&name,&key,&kind,&payload,&saved)
    }).await.map_err(|_|failure("sidecar_job"))?
}
/// `session.patch_meta`/`session.clear_cano` de uma sessão sem terminal, gravados no sidecar do provedor.
/// Bloqueante: chamar fora do runtime.
pub(super) fn sidecar_service(provider:&str,dir:&std::path::Path,name:&str,key:&str,kind:&str,payload:&Value,view:&Value) -> Result<Value,RuntimeError> {
    match (provider == "claude",kind == "session.clear_cano") {
        (true,true)=>super::claude_sidecar::clear_cano(dir,name,key,payload),
        (true,false)=>super::claude_sidecar::patch(dir,name,key,payload,view),
        (false,true)=>super::codex_sidecar::clear_cano(dir,name,key,payload),
        (false,false)=>super::codex_sidecar::patch_headless(dir,name,key,payload,view),
    }
}
/// `runtime_state.view` do arquivo da fila, lido como o `session.patch_meta` do Python lia.
fn saved_view(state_path:&std::path::Path) -> Result<Value,RuntimeError> {
    let state:Value = super::queue::read_replaced(state_path).ok().and_then(|raw|serde_json::from_slice(&raw).ok()).ok_or_else(||failure("queue_state_read"))?;
    Ok(match &state["runtime_state"]["view"] { Value::Null=>json!({}),view=>view.clone() })
}
async fn clear_cano(target:&RuntimeTarget,sidecar_dir:&std::path::Path,pid:u32) {
    if let Err(error) = launch_sidecar(target,sidecar_dir,"session.clear_cano",json!({"pid":pid})).await {
        tracing::warn!(key=%target.key,code=%error.code,"cano não saiu do arquivo da sessão");
    }
}
/// O cano subido agora e que não deu conexão: morre e sai do arquivo da sessão.
pub(super) async fn discard(target:&RuntimeTarget,cano:&process::Cano,sidecar_dir:&std::path::Path) {
    if let Err(stop) = process::kill(cano,&target.key,sidecar_dir).await {
        tracing::warn!(key=%target.key,code=stop.code(),"cano sem conexão não foi encerrado");
        return;
    }
    clear_cano(target,sidecar_dir,cano.pid).await;
}

/// Prazo da devolução da janela esticada ao abrir a sessão com terminal (`unstretch`).
const UNSTRETCH_MAX:Duration = Duration::from_secs(2);

/// Interface dos mods da sessão com terminal (fase 3): o Rust é o dono do terminal aqui, então é dono do
/// clique e da faixa dela. Liga a sessão ao `Mods` na vida `life` com um elo novo; o processo é a chave
/// durável mais o pane e a criação dele, que o renomear mantém.
async fn attach_terminal_mods(mods:&crate::mods::state::Mods,target:&super::terminal::TerminalTarget,
    handle:&super::terminal::TerminalHandle,life:u64) {
    // Antes de ligar: nenhum pedido de app pode estar esticando a janela enquanto ela é lida.
    crate::mods::click::unstretch(handle,std::time::Instant::now()+UNSTRETCH_MAX).await;
    let link = crate::mods::terminal::TerminalLink::anchored(target.name.clone(),life,Arc::new(handle.clone()),mods.clone(),
        crate::mods::click::Limits::default(),handle.anchor());
    let process = format!("{}:{}:{}",target.key,target.binding.pane,target.binding.created);
    mods.attach_terminal_keyed(&target.name,&process,target.plugin_key.as_deref(),life,link.clone());
    // O vigia sobe depois de ligar: o `attach_terminal` para o vigia do elo que estava no nome, e se fosse
    // este mesmo elo o derrubaria. No psmux não há vigia (o `watch_notices` recusa no Windows): lá o mínimo
    // volta na operação seguinte, no `prepare` do clique. O mínimo vale já na abertura: a janela pode ter
    // ficado pequena com um terminal que se desligou.
    if !target.binding.windows { link.watch(&target.binding.mux_argv); }
    tokio::spawn(async move { link.floor().await; });
}

impl RuntimeRegistry {
    pub fn new(upstream:SocketAddr,secret:String,instance:String) -> Self {
        Self { diag:crate::diag::DiagClient::new(upstream,secret.clone()),reopening:Default::default(),terminal_problems:Default::default(),terminal_status:Default::default(),heal_attempts:Default::default(),interactions:None,entries:Arc::new(Mutex::new(BTreeMap::new())),terminal_states:None,terms:None,events:broadcast::channel(1024).0,
            policy:PolicyClient::new(upstream,secret),instance,lifecycle:Mutex::new(BTreeMap::new()),revisions:Mutex::new(BTreeMap::new()),mods:None,plugin:None,ingress:Default::default(),
            respawn_base:Duration::from_secs(5),reconnect_waits:Default::default(),permissions:None,processes:Arc::new(crate::list::procs::SystemProcs::default()),live:Default::default(),codex_owned:Default::default(),hubs:std::sync::OnceLock::new(),self_ref:std::sync::OnceLock::new(),
            watcher:std::sync::OnceLock::new(),codex_wake:Arc::new(tokio::sync::Notify::new()),native:Default::default(),limits:std::sync::OnceLock::new(),limit_holds:Default::default() }
    }
    /// Escrita para sessão doente, ou a conversa aberta: a sessão em erro é reaberta aqui, em segundo plano
    /// e uma por vez; a saudável só refaz a projeção da fila. Responde na hora; quem escreveu recebe o 503
    /// e tenta de novo. Falha vai ao diário.
    /// Liga a troca de conta no limite; sem ela o aviso de limite só fica no estado da sessão.
    pub fn set_limit_sink(&self,sink:tokio::sync::mpsc::UnboundedSender<LimitHit>) { let _ = self.limits.set(sink); }
    /// A fila do Claude para já, no mesmo passo do aviso: o que estiver nela iria para a conta esgotada.
    /// `true`: o vigia recebeu. Sem vigia vai ao diário.
    pub fn usage_limit(&self,hit:LimitHit) -> bool {
        let Some(sink) = self.limits.get() else {
            tracing::warn!(session=%hit.name,"aviso de limite sem o vigia da troca de conta");
            self.diag.report("runtime.limit_sink_closed",&hit.name,"limit_sink_absent","o vigia da troca de conta não existe; o aviso de limite se perdeu");
            return false;
        };
        let name = hit.name.clone();
        if hit.provider == "claude" { self.limit_holds.lock().unwrap_or_else(|e|e.into_inner()).insert(name.clone()); }
        // Segurada antes de enviar (a troca pode soltar antes de esta linha voltar); sem o vigia, ninguém soltaria.
        if sink.send(hit).is_err() {
            self.limit_holds.lock().unwrap_or_else(|e|e.into_inner()).remove(&name);
            tracing::warn!(session=%name,"aviso de limite sem o vigia da troca de conta; a fila segue entregando");
            self.diag.report("runtime.limit_sink_closed",&name,"limit_sink_closed","o vigia da troca de conta não recebeu o aviso de limite");
            return false;
        }
        true
    }
    pub fn limit_held(&self,name:&str) -> bool { self.limit_holds.lock().unwrap_or_else(|e|e.into_inner()).contains(name) }
    /// Fim da troca (feita ou não): a fila volta a entregar, a começar pelo recado de continuar.
    /// A drenagem corre à parte: ela só responde quando a entrega termina, e a troca não espera o turno.
    pub fn release_limit(&self,name:&str) {
        if !self.limit_holds.lock().unwrap_or_else(|e|e.into_inner()).remove(name) { return; }
        let Some(registry) = self.self_ref.get().and_then(std::sync::Weak::upgrade) else { return };
        let name = name.to_owned();
        tokio::spawn(async move {
            if let Err(error) = registry.drain(&name).await { tracing::warn!(session=%name,code=%error.code,"fila solta depois do limite não drenou agora"); }
        });
    }
    pub fn request_reopen(&self,name:&str) {
        let Some(registry) = self.self_ref.get().and_then(std::sync::Weak::upgrade) else { return };
        if !self.reopening.lock().unwrap_or_else(|e|e.into_inner()).insert(name.to_owned()) { return; }
        let name = name.to_owned();
        tokio::spawn(async move {
            let result = registry.reopen_if_failed(&name).await;
            registry.reopening.lock().unwrap_or_else(|e|e.into_inner()).remove(&name);
            match result {
                Ok(true)=>{
                    tracing::warn!(session=%name,"sessão em erro reaberta");
                    registry.diag.report("runtime.reopened",&name,"reopened","sessão em erro reaberta no Rust");
                },
                Ok(false)=>{},
                Err(error)=>{
                    tracing::warn!(session=%name,code=%error.code,"sessão em erro não reabriu");
                    registry.diag.report("runtime.reopen_failed",&name,&error.code,"a sessão em erro não reabriu no Rust");
                },
            }
        });
    }
    /// `true` quando reabriu. Uma tentativa por pedido; a de outra operação no meio (fechar, trocar de
    /// modo) vence, pela barreira da chave.
    async fn reopen_if_failed(&self,name:&str) -> Result<bool,RuntimeError> {
        // Uma troca em curso (porta fechada) é quem decide a vida da sessão: nada a reabrir por cima dela.
        let Ok(_pass) = self.ingress.enter(name,Duration::ZERO).await else { return Ok(false) };
        let found = self.entries.lock().await.iter().find(|(_,entry)|entry.name == name)
            .map(|(key,entry)|(key.clone(),entry.generation,entry.handle.clone(),entry.provider.clone()));
        let Some((key,generation,handle,provider)) = found else {
            // A vida que fechou aqui (ou não abriu) com o processo de pé: o Claude sem terminal volta pelo sidecar.
            if self.runtime_lost(name).await.is_none() { return Ok(false); }
            return self.open_claude_headless(name,true).await.transpose().map(|opened|opened.is_some());
        };
        // Conversa aberta: a religação ao cano já marcada (Claude e Codex sem terminal) é antecipada, sem abrir outra.
        if let EntryHandle::Headless(actor) = &handle { actor.wake_reconnect(); }
        let terminal = matches!(handle,EntryHandle::Terminal {..});
        if handle.snapshot().await.is_ok_and(|snapshot|healthy(terminal,&snapshot)) {
            handle.ensure_projection().await?;
            return Ok(false);
        }
        {
            let barrier = self.barrier(&key).await;
            let _guard = barrier.lock().await;
            self.close_locked(&key,generation,false).await?;
        }
        let watcher = || self.watcher.get().cloned().ok_or_else(||failure("runtime_binding"));
        let reopened = match handle {
            // Agente desta vida morto: reabrir o mesmo alvo ressuscitaria pane, pid e conversa mortos; a vida
            // que estiver no nome agora é adotada.
            EntryHandle::Terminal {target,..} if super::lifecycle::agent_gone(name,
                i64::from(target.metadata.agent_pid),target.metadata.agent_birth)=>match watcher()?.adopt_claude(self,name).await {
                None=>return Ok(false),
                adopted=>adopted,
            },
            EntryHandle::Terminal {target,..}=>Some(self.open_terminal(target).await.map(|_|())),
            EntryHandle::CodexTerminal {..}=>watcher()?.adopt_now(self,name).await,
            EntryHandle::Headless(_) if provider == "codex"=>watcher()?.open_parked(self,name).await,
            EntryHandle::Headless(_)=>self.open_claude_headless(name,true).await,
        };
        reopened.ok_or_else(||failure("runtime_binding"))??;
        Ok(true)
    }
    /// Liga a vigia do Codex com terminal: escrita sem ator adota na hora e encerrar passa por ela.
    pub fn bind_watcher(&self,watcher:Arc<super::codex_terminal::Watcher>) { let _ = self.watcher.set(watcher); }
    pub fn codex_wake(&self) -> Arc<tokio::sync::Notify> { self.codex_wake.clone() }
    /// Escrita para uma sessão sem ator: o Claude sem terminal abre aqui (subindo o processo parado, como o
    /// `ensure_running` do Python antes de toda operação) e, com a vigia ligada, o Codex é adotado agora.
    /// `None` = não é sessão sem ator do Rust.
    pub async fn adopt_now(&self,name:&str) -> Option<Result<(),RuntimeError>> {
        if let Err(error) = self.retire_other_life(name).await { return Some(Err(error)); }
        // Pela porta, como as escritas: no meio de uma troca (porta fechada) abrir ou adotar subiria a vida
        // antiga por cima da que está nascendo.
        let _pass = match self.ingress.enter(name,Duration::ZERO).await {
            Ok(pass)=>pass,
            Err(_)=>return Some(Err(RuntimeError::new("session_transfer_busy","a sessão está trocando de agente"))),
        };
        if let Some(opened) = self.open_claude_headless(name,true).await { return Some(opened); }
        let watcher = self.watcher.get()?.clone();
        if let Some(adopted) = watcher.adopt_now(self,name).await { return Some(adopted); }
        watcher.adopt_claude(self,name).await
    }
    pub(super) fn report(&self,event:&'static str,name:&str,code:&str,reason:&'static str) { self.diag.report(event,name,code,reason); }
    pub(super) fn watcher_ref(&self) -> Option<Arc<super::codex_terminal::Watcher>> { self.watcher.get().cloned() }
    pub(super) fn policy_plugin_bridge(&self) -> Option<crate::session_open::claude::PluginBridge> { self.policy.plugin_bridge() }
    pub(super) fn live_channel(&self,name:&str) -> LiveSender { self.live_sender(name) }
    /// O retrato confirmado do modo da conversa (memória do Rust), sem leitura incerta.
    pub(super) fn permission_view(&self,conversation:&str) -> Option<crate::state::permission::PermissionView> {
        self.permissions.as_ref()?.lock().ok()?.get(conversation)
    }
    /// Geração e vista do Claude sem terminal aberto aqui com esse nome.
    pub(super) async fn claude_headless_view(&self,name:&str) -> Option<(u64,Value)> {
        let found=self.entries.lock().await.values().find_map(|entry|match &entry.handle {
            EntryHandle::Headless(handle) if entry.name==name && entry.provider=="claude"=>Some((entry.generation,handle.clone())),
            _=>None,
        });
        let (generation,handle)=found?;
        handle.view().await.ok().map(|view|(generation,view))
    }
    /// Alvo e ator do Claude com terminal aberto aqui com esse nome.
    pub(super) async fn claude_terminal_entry(&self,name:&str) -> Option<(super::terminal::TerminalTarget,super::terminal::TerminalHandle)> {
        self.entries.lock().await.values().find_map(|entry|match &entry.handle {
            EntryHandle::Terminal {target,handle} if entry.name==name && target.generation==entry.generation=>Some((target.clone(),handle.clone())),
            _=>None,
        })
    }
    /// Fecha a entrada de `key` só se o ator dela ainda estiver morto, conferido dentro da barreira: outra
    /// abertura concorrente pode já ter posto um ator vivo com a mesma geração. `true`: há ator vivo.
    async fn close_if_dead(&self,key:&str) -> Result<bool,RuntimeError> {
        let barrier=self.barrier(key).await;let _guard=barrier.lock().await;
        let Some((generation,handle))=self.entries.lock().await.get(key).map(|entry|(entry.generation,entry.handle.clone())) else {return Ok(false)};
        if handle.snapshot().await.is_ok() {return Ok(true)}
        self.close_locked(key,generation,false).await.map(|_|false)
    }
    /// Fecha a entrada sob a barreira da chave; o processo segue vivo.
    pub(super) async fn close_claude_entry(&self,key:&str,generation:u64) -> Result<Value,RuntimeError> {
        let barrier=self.barrier(key).await;let _guard=barrier.lock().await;
        self.close_locked(key,generation,false).await
    }
    /// Chave, instante em que nasceu a sessão do multiplexador e agente (pid, nascimento) da entrada com
    /// terminal de `name`.
    pub(super) async fn terminal_life(&self,name:&str) -> Option<(String,u64,i64,f64)> {
        self.entries.lock().await.iter().find_map(|(key,entry)|match &entry.handle {
            EntryHandle::Terminal {target,..} if entry.name==name=>Some((key.clone(),target.binding.created,
                i64::from(target.metadata.agent_pid),target.metadata.agent_birth)),
            _=>None,
        })
    }
    /// A entrada com terminal `key` de `name`, nascida em `created`, é de outra vida do nome: sai daqui sem
    /// matar nada. O instante também é conferido: a reabertura (conta, `/recarregar`) herda a chave, e a
    /// entrada da vida nova com ela nunca sai por aqui.
    pub(super) async fn retire_terminal(&self,name:&str,key:&str,created:u64) -> Result<(),RuntimeError> {
        let found=self.entries.lock().await.get(key).filter(|entry|entry.name==name
            && matches!(&entry.handle,EntryHandle::Terminal {target,..} if target.binding.created==created)).map(|entry|entry.generation);
        let Some(generation)=found else { return Ok(()) };
        tracing::warn!(session=%name,key,code="runtime_stale_life","entrada de outra vida da sessão saiu do registro");
        self.diag.report("runtime.stale_life",name,"runtime_stale_life","a entrada guardada era de uma vida anterior da sessão");
        self.close_claude_entry(key,generation).await.map(|_|())
    }
    /// Criação de sessão com `name`, já conferido livre (sem pane nem sidecar): toda entrada guardada com
    /// ele é de uma vida que acabou e sai daqui sem matar nada, antes de a vida nova nascer.
    pub async fn release_name(&self,name:&str) -> Result<(),RuntimeError> {
        let stale:Vec<(String,u64)>=self.entries.lock().await.iter().filter(|(_,entry)|entry.name==name)
            .map(|(key,entry)|(key.clone(),entry.generation)).collect();
        for (key,generation) in stale {
            tracing::warn!(session=%name,key,code="runtime_stale_life","entrada de uma vida que acabou saiu do registro na criação");
            self.diag.report("runtime.stale_life",name,"runtime_stale_life","a entrada guardada era de uma vida anterior da sessão");
            self.close_claude_entry(&key,generation).await?;
        }
        Ok(())
    }
    /// Antes de pegar a porta: a entrada com terminal guardada para `name` que é de outra vida sai daqui
    /// (`Watcher::retire_other_life`). Depois da porta, ela disputaria a barreira com o `dead` da vida velha.
    pub async fn retire_other_life(&self,name:&str) -> Result<(),RuntimeError> {
        match self.watcher.get().cloned() { Some(watcher)=>watcher.retire_other_life(self,name).await, None=>Ok(()) }
    }
    /// `DELETE` do Claude aberto aqui, com ou sem terminal: o Rust encerra a vida que guarda. O `Ok` leva o
    /// `transcript`. `None` = não há Claude com esse nome aqui, ou a vida guardada já não era a do nome (a
    /// entrada sai e o Python fecha pelo nome, como antes).
    pub async fn kill_claude(&self,name:&str) -> Option<Result<Value,RuntimeError>> {
        if let Err(error)=self.retire_other_life(name).await { return Some(Err(error)); }
        let found=self.entries.lock().await.iter().find_map(|(key,entry)|match &entry.handle {
            EntryHandle::Terminal {target,..} if entry.name==name=>Some((key.clone(),entry.generation,target.transcript.clone(),
                Some((target.binding.mux_argv.clone(),i64::from(target.metadata.agent_pid),target.metadata.agent_birth)))),
            EntryHandle::Headless(_) if entry.name==name && entry.provider=="claude"=> {
                // A conversa viva (depois de um `/clear` no processo), como em `context_transcript`.
                let conversation=entry.live.borrow().as_ref().and_then(|live|live.binding.as_ref().map(|binding|binding.conversation.clone())).filter(|c|!c.is_empty());
                let transcript=match conversation { Some(c)=>entry.mode_source.transcript.with_file_name(format!("{c}.jsonl")), None=>entry.mode_source.transcript.clone() };
                Some((key.clone(),entry.generation,transcript,None))
            },
            _=>None,
        });
        let (key,generation,transcript,terminal)=found?;
        // Agente desta vida já morto: nada a matar aqui; a entrada sai e o Python fecha pelo nome.
        if let Some((_,pid,birth))=&terminal && super::lifecycle::agent_gone(name,*pid,*birth) {
            return self.release_unproven(name,&key,generation,"lifecycle_agent_gone").await;
        }
        let mux_argv=terminal.map(|(argv,_,_)|argv);
        let result=self.close_with_kill_as(&key,generation,true).await;
        let gone=!self.entries.lock().await.contains_key(&key);
        match result {
            Ok(mut value) if value["killed"]==json!(true)=>{
                if let Some(program)=mux_argv.as_ref().and_then(|argv|argv.first()) {
                    // O shell do painel (`term-<nome>`) morre junto, só com a marca de que é nosso.
                    let mux=crate::state::capture::MuxProcess::new(program,crate::state::capture::TIMEOUT);
                    let hidden=format!("term-{name}");
                    if mux.is_hidden(&hidden).await && mux.kill_session(&hidden).await!=Some(true) {
                        tracing::warn!(session=%name,"shell escondido do Claude com terminal não fechou");
                    }
                }
                if !transcript.as_os_str().is_empty() { value["transcript"]=json!(transcript.to_string_lossy()); }
                Some(Ok(value))
            },
            // O ator saiu, mas o processo não é comprovadamente desta vida: nada foi morto aqui.
            Ok(_) if gone=>{
                tracing::warn!(session=%name,key=%key,code="close_killed_false","processo do Claude não comprovado desta vida; o fechamento segue pelo nome");
                None
            },
            Err(error) if gone=>{
                tracing::warn!(session=%name,key=%key,code=%error.code,"vida guardada do Claude saiu com erro; o fechamento segue pelo nome");
                None
            },
            // A vida guardada não se comprova (pane, nascimento ou vínculo): a entrada sai sem matar nada e o
            // Python fecha pelo nome, como antes.
            Err(error) if LIFE_UNPROVEN.contains(&error.code.as_str())=>self.release_unproven(name,&key,generation,&error.code).await,
            other=>Some(other),
        }
    }
    /// A entrada de uma vida que não se comprova sai do registro sem matar nada e vai ao diário; `None`
    /// devolve o fechamento ao Python, pelo nome. Se nem sair ela consegue, o erro volta a quem pediu.
    async fn release_unproven(&self,name:&str,key:&str,generation:u64,code:&str) -> Option<Result<Value,RuntimeError>> {
        tracing::warn!(session=%name,key,code,"vida guardada do Claude não se comprova; sai do registro e o fechamento segue pelo nome");
        self.diag.report("runtime.stale_life",name,"runtime_stale_life","a vida guardada não se comprovou no fechamento");
        match self.close_claude_entry(key,generation).await {
            Ok(_)=>None,
            Err(_) if !self.entries.lock().await.contains_key(key)=>None,
            Err(error)=>Some(Err(error)),
        }
    }
    /// `DELETE` que entra pelo Python: `None` = não é Codex com terminal do Rust.
    pub async fn kill_codex_terminal(&self,name:&str) -> Option<Result<Value,RuntimeError>> {
        let watcher = self.watcher.get()?.clone();
        watcher.kill_by_name(self,name).await
    }
    /// Troca terminal ↔ sem terminal do Codex do Rust, pela vigia. `None` = não é do Rust (o Python troca).
    pub async fn relaunch_codex(&self,name:&str,terminal:bool,panels:Option<Arc<crate::term::Terms>>) -> Option<Result<Value,RuntimeError>> {
        let watcher = self.watcher.get()?.clone();
        watcher.switch_mode(self,name,terminal,super::codex_relaunch::PermissionRead::Pane(panels)).await
    }
    /// Sobe o Codex sem terminal que acabou de ganhar sidecar. `None` = vigia desligada ou sidecar que
    /// não é sem terminal do Rust.
    pub async fn open_codex_headless(&self,name:&str) -> Option<Result<(),RuntimeError>> {
        let watcher = self.watcher.get()?.clone();
        watcher.open_parked(self,name).await
    }
    /// Reiniciar o Codex do Rust. `None` = não é do Rust ou há transferência a recuperar (o Python atende).
    pub async fn restart_codex(&self,name:&str) -> Option<Result<Value,RuntimeError>> {
        let watcher = self.watcher.get()?.clone();
        watcher.restart(self,name).await
    }
    /// Entrada sem terminal do Codex aberta aqui para `name`: chave, geração e ator.
    pub(crate) async fn codex_headless(&self,name:&str) -> Option<(String,u64,RuntimeHandle)> {
        self.entries.lock().await.iter().find_map(|(key,entry)|match &entry.handle {
            EntryHandle::Headless(handle) if entry.name == name && entry.provider == "codex"=>Some((key.clone(),entry.generation,handle.clone())),
            _=>None,
        })
    }
    /// A ponte do Codex atende também o sem terminal do Rust: escrita do convidado não passa pelo slot Python.
    pub async fn owns_codex_headless(&self,name:&str) -> bool { self.codex_headless(name).await.is_some() }
    /// Pasta dos sidecars do Claude sem terminal e da fila, pelo ambiente-base (o de teste, se injetado).
    fn claude_dirs(&self) -> (std::path::PathBuf,std::path::PathBuf) {
        let env = self.policy.codex_environment();
        (env.home.join(".hangar").join("claude-headless"),env.claude_base.join(".hangar-queue"))
    }
    /// Na subida: abre aqui cada Claude sem terminal com cano vivo da chave (adota) ou com mensagem
    /// ainda não entregue na fila (sobe o processo); a parada sem pendência fica fechada e o próximo envio
    /// a sobe. Falha de uma não impede as outras e vai ao diário.
    pub async fn register_claude_headless(self:Arc<Self>) {
        let (dir,queue_dir) = self.claude_dirs();
        let listed = tokio::task::spawn_blocking(move || -> std::io::Result<(Vec<(String,bool,bool)>,Vec<String>)> {
            let (mut out,mut unreadable) = (Vec::new(),Vec::new());
            let entries = match std::fs::read_dir(&dir) { Ok(entries)=>entries,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok((out,unreadable)),Err(error)=>return Err(error) };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|ext|ext != "json") { continue; }
                let meta = match super::codex_sidecar::read(&path) {
                    Ok(Some(meta))=>meta,
                    Ok(None)=>continue,
                    Err(_)=>{ unreadable.push(path.file_stem().map(|stem|stem.to_string_lossy().into_owned()).unwrap_or_default()); continue; },
                };
                let (Some(name),Some(key)) = (meta.get("name").and_then(Value::as_str),meta.get("key").and_then(Value::as_str).filter(|k|!k.is_empty())) else { continue };
                let alive = meta.get("cano").and_then(|cano|cano["pid"].as_u64()).and_then(|pid|u32::try_from(pid).ok())
                    .is_some_and(|pid|process::liveness(pid,key)==process::Liveness::Ours);
                out.push((name.to_owned(),alive,pending(&super::codex_terminal::queue_files(&queue_dir,key).0)));
            }
            Ok((out,unreadable))
        }).await;
        let listed = match listed {
            Ok(Ok((listed,unreadable)))=>{
                for name in unreadable {
                    tracing::warn!(session=%name,"sidecar do Claude sem terminal ilegível na subida");
                    self.diag.report("runtime.registration_failed",&name,"claude_sidecar_read","o sidecar da sessão sem terminal existe e não foi lido na subida");
                }
                listed
            },
            Ok(Err(error))=>{
                tracing::warn!(kind=?error.kind(),"sidecars do Claude sem terminal ilegíveis na subida");
                self.diag.report("runtime.registration_failed","*","sidecar_dir","a pasta dos sidecars do Claude sem terminal não foi lida");
                return;
            },
            Err(_)=>return,
        };
        let opens = listed.into_iter().filter(|(_,alive,pending)|*alive || *pending).map(|(name,_,pending)| {
            let registry = self.clone();
            async move {
                if let Some(Err(error)) = registry.open_claude_headless(&name,pending).await {
                    tracing::warn!(session=%name,code=%error.code,"sessão sem terminal não abriu na subida");
                    registry.diag.report("runtime.registration_failed",&name,&error.code,"a sessão sem terminal não abriu na subida do Rust");
                }
            }
        });
        futures_util::future::join_all(opens).await;
    }
    /// Estaciona cada Claude sem terminal parado há `after` sem nada em aberto: o processo morre, o sidecar e a
    /// fila ficam, e o próximo envio sobe outro pelo spec (`adopt_now`).
    pub async fn park_idle_claude(&self,after:Duration) {
        let candidates:Vec<(String,u64,String)> = self.entries.lock().await.iter()
            .filter(|(_,entry)|entry.provider=="claude" && matches!(entry.handle,EntryHandle::Headless(_)) && entry.auto_respawn.is_some())
            .map(|(key,entry)|(key.clone(),entry.generation,entry.name.clone())).collect();
        for (key,generation,name) in candidates {
            match self.park_claude(&key,generation,&name,after).await {
                Ok(None)=>{},
                Ok(Some(idle))=>{
                    tracing::info!(session=%name,idle_s=idle as u64,"sessão sem terminal parada estacionada");
                    let event=json!({"evento":"sessao.estacionada","nivel":"ok","sessao":name,"provider":"claude","parada_s":idle as u64});
                    crate::diag::record(crate::diag::diary_dir_in(&self.policy.codex_environment()),vec![event],"estacionamento");
                },
                Err(error)=>{
                    tracing::warn!(session=%name,code=%error.code,"sessão parada não foi estacionada");
                    self.diag.report("runtime.park_failed",&name,&error.code,"a sessão sem terminal parada não foi estacionada");
                },
            }
        }
    }
    /// `_pode_estacionar`: o motor sem nada em aberto há `after`, nenhuma escrita nem operação de ciclo em curso e
    /// a fila toda entregue, conferido de novo com a porta fechada. `Some(segundos parada)` = estacionou.
    async fn park_claude(&self,key:&str,generation:u64,name:&str,after:Duration) -> Result<Option<f64>,RuntimeError> {
        let idle=|view:&Value|view["park_idle_s"].as_f64().filter(|idle|*idle>=after.as_secs_f64() && view["lifecycle_pending"]!=true);
        let Some(handle)=self.entries.lock().await.get(key).filter(|entry|entry.generation==generation).map(|entry|entry.handle.clone()) else {return Ok(None)};
        if idle(&handle.view().await?).is_none() {return Ok(None);}
        let Ok(_ingress)=super::lifecycle::ClosedIngress::acquire_within(&self.ingress,name,Duration::from_secs(1)).await else {return Ok(None)};
        let barrier=self.barrier(key).await;
        if barrier.operation.lock().map_err(|_|failure("lifecycle_operation"))?.is_some() {return Ok(None);}
        let Ok(_guard)=barrier.try_lock() else {return Ok(None)};
        let Ok(_parking)=barrier.park.try_write() else {return Ok(None)};
        let found=self.entries.lock().await.get(key).filter(|entry|entry.generation==generation && !entry.respawn_inflight)
            .and_then(|entry|Some((entry.runtime_target.clone()?,entry.cano_birth?)));
        let Some((target,born))=found else {return Ok(None)};
        let Some(idle)=idle(&handle.view().await?) else {return Ok(None)};
        let state_path=target.state_path.clone();
        if tokio::task::spawn_blocking(move||pending(&state_path)).await.map_err(|_|failure("queue_job"))? {return Ok(None);}
        let procs=self.processes.clone();
        if !cano_is_ours(procs.as_ref(),&target,born,key)? {return Err(failure("lifecycle_process_changed"));}
        self.close_locked_as(key,generation,true,true).await?;
        *barrier.parked.lock().map_err(|_|failure("lifecycle_parked"))?=Some(name.to_owned());
        Ok(Some(idle))
    }
    /// `_vigiar_ociosas`: uma rodada por minuto enquanto o registro existir.
    pub async fn watch_idle_claude(self:Arc<Self>) {
        let registry=Arc::downgrade(&self);
        drop(self);
        loop {
            tokio::time::sleep(PARK_ROUND).await;
            let Some(registry)=registry.upgrade() else {break};
            registry.park_idle_claude(PARK_AFTER).await;
        }
    }
    /// Claude sem terminal sem ator aqui: o Rust o abre pelo sidecar dele, adotando o cano vivo da chave
    /// ou, com `launch`, subindo um pelo spec. `None`: não há sessão Claude sem terminal com esse nome.
    pub async fn open_claude_headless(&self,name:&str,launch:bool) -> Option<Result<(),RuntimeError>> {
        let (dir,queue_dir) = self.claude_dirs();
        let path = super::codex_sidecar::path(&dir,name);
        let meta = match tokio::task::spawn_blocking(move||super::codex_sidecar::read(&path)).await {
            Ok(Ok(meta))=>meta?,
            failed=>{
                if let Ok(Err(error)) = &failed { tracing::warn!(session=%name,kind=?error.kind(),"sidecar do Claude sem terminal ilegível"); }
                self.diag.report("runtime.open_failed",name,"claude_sidecar_read","o sidecar da sessão sem terminal existe e não foi lido");
                return Some(Err(RuntimeError::new("claude_sidecar_read","o arquivo da sessão sem terminal não pôde ser lido")));
            },
        };
        let key = meta.get("key").and_then(Value::as_str).filter(|key|!key.is_empty())?.to_owned();
        let open = self.entries.lock().await.get(&key).filter(|entry|entry.name == name).map(|entry|entry.handle.clone());
        if let Some(handle) = open {
            if handle.snapshot().await.is_ok() { return Some(Ok(())); }
            // Ator morto não é sessão aberta: a entrada sai sem matar nada e a sessão abre de verdade.
            match self.close_if_dead(&key).await { Ok(true)=>return Some(Ok(())),Ok(false)=>{},Err(error)=>return Some(Err(error)) }
        }
        // Entrada com terminal de outra vida (o Claude que saiu deste nome) não barra a sem terminal.
        if let Err(error) = self.retire_other_life(name).await { return Some(Err(error)); }
        if self.has_entry(Some(&key),name).await {
            return Some(Err(RuntimeError::new("claude_headless_entry_conflict","já há outra sessão aberta aqui com este nome ou chave")));
        }
        let (state_path,lease_path) = super::codex_terminal::queue_files(&queue_dir,&key);
        // Sem a trava da fila: outra abertura desta sessão pode estar em curso (e o ator a segura a vida
        // toda); quem decide entre as duas é a barreira do `open_attempt`, que acha a sessão já aberta.
        let generation = {
            let state_path = state_path.clone();
            let read = tokio::task::spawn_blocking(move || match super::queue::read_replaced(&state_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(1),
                Err(error) => Err(error),
                Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().and_then(|state|state["generation"].as_u64())
                    .ok_or_else(||std::io::Error::from(std::io::ErrorKind::InvalidData)),
            }).await;
            match read {
                Ok(Ok(generation))=>generation,
                failed=>{
                    let error = match failed { Ok(Err(error))=>RuntimeError::new("queue_io",&format!("fila da sessão inacessível: {:?}",error.kind())),_=>failure("queue_job") };
                    super::actor::mark_live_error(&self.live_sender(name),&error.code,&error.message);
                    return Some(Err(error));
                },
            }
        };
        let mut metadata = Value::Object(meta.clone());
        metadata["key"] = json!(key);
        let env = self.policy.codex_environment();
        let transcript = crate::session_open::record::claude_from_meta(&metadata,name,0.0).ok()
            .zip(metadata["session_id"].as_str()).map(|(spec,sid)|super::claude_launch::transcript(&env,&spec,sid)).unwrap_or_default();
        let cano = meta.get("cano").filter(|cano|cano.is_object());
        let field = |name:&str|cano.and_then(|cano|cano[name].as_str()).unwrap_or_default().to_owned();
        // Cano vivo da sessão sem `versao` gravada já fala a 2.
        let binding = CanoBinding { pid:cano.and_then(|cano|cano["pid"].as_u64()).and_then(|pid|u32::try_from(pid).ok()).unwrap_or(0),
            escuta:field("escuta"),token:field("token"),versao:cano.and_then(|cano|cano["versao"].as_u64()).and_then(|v|u32::try_from(v).ok()).unwrap_or(2) };
        let target = RuntimeTarget { key,generation,name:name.into(),provider:"claude".into(),binding,transcript,
            created:meta.get("created").and_then(Value::as_f64).unwrap_or(0.0),metadata,lease_path,state_path,projection_dir:queue_dir };
        // A vida fechada aqui com outro processo de pé (o Claude que sobreviveu ao cano): subir outro seriam dois
        // na mesma conversa. O cano gravado no sidecar é o que a abertura adota, não conta.
        let closed=self.barrier(&target.key).await.closed.lock().ok().and_then(|closed|closed.clone());
        if let Some(closed)=closed && closed.pid!=i64::from(target.binding.pid)
            && !super::lifecycle::process_absent(&crate::list::procs::SystemProcs::default(),closed.pid,closed.birth).unwrap_or(false) {
            return Some(Err(RuntimeError::new("lifecycle_agent_still_alive","o Claude anterior desta sessão ainda está vivo; nada novo sobe por cima dele")));
        }
        Some(if launch { self.open_with_launch(target,dir).await } else { self.open_managed(target,dir).await }.map(|_|()))
    }
    /// Ambiente-base do Codex (o de teste, se injetado), para a troca de modo montar lançador e conta.
    pub(crate) fn codex_environment(&self) -> crate::accounts::environment::AccountEnvironment { self.policy.codex_environment() }
    /// Alvos dos atores do Codex com terminal abertos aqui, para a vigia.
    pub async fn codex_terminals(&self) -> Vec<CodexTerminalTarget> {
        self.entries.lock().await.values().filter_map(|entry|match &entry.handle {
            EntryHandle::CodexTerminal { target,.. } if target.generation == entry.generation=>Some((**target).clone()),
            _=>None,
        }).collect()
    }
    /// Alguma entrada aberta com esta chave ou este nome, de qualquer provedor.
    pub async fn has_entry(&self,key:Option<&str>,name:&str) -> bool {
        self.entries.lock().await.iter().any(|(k,entry)|Some(k.as_str()) == key || entry.name == name)
    }
    /// Sessão sem entrada aqui que nunca foi do Rust (o `claude` aberto à mão, que só entra no primeiro envio
    /// pelo app). A que o Rust não abriu (erro no canal da sessão) ou fechou com o processo ainda vivo não é:
    /// essa segue com o código (`runtime_lost`).
    pub async fn hand_opened(&self,name:&str) -> bool {
        !self.has_entry(None,name).await && self.runtime_lost(name).await.is_none()
    }
    /// Código da sessão do Rust que ficou sem entrada: a abertura que falhou, ou a vida fechada aqui cujo
    /// processo continua de pé.
    pub async fn runtime_lost(&self,name:&str) -> Option<String> {
        let failed=self.live.lock().unwrap_or_else(|e|e.into_inner()).get(name)
            .and_then(|sender|sender.borrow().as_ref().and_then(|state|state.error.as_ref().map(|error|error.0.clone())));
        if failed.is_some() {return failed;}
        let barriers:Vec<_>=self.lifecycle.lock().await.values().cloned().collect();
        let procs=crate::list::procs::SystemProcs::default();
        barriers.iter().filter_map(|barrier|barrier.closed.lock().ok().and_then(|closed|closed.clone()))
            .find(|closed|closed.name==name && !super::lifecycle::process_absent(&procs,closed.pid,closed.birth).unwrap_or(false))
            .map(|_|"runtime_binding".to_owned())
    }
    /// Liga os hubs do chat: abrir e fechar o ator do Codex com terminal troca o dono do estado neles.
    pub fn bind_hubs(self:&Arc<Self>,hubs:crate::side::WeakHubs) {
        if self.hubs.set(hubs).is_err() {return;}
        let weak=Arc::downgrade(self);let mut events=self.events.subscribe();
        tokio::spawn(async move {
            loop {
                let event=match events.recv().await {Ok(event)=>event,Err(broadcast::error::RecvError::Lagged(_))=>{tracing::warn!(code="lifecycle_event_lagged","ligação de conversa perdeu eventos");continue;},Err(_)=>break};
                let Some(registry)=weak.upgrade() else {break};
                if event.channel=="snapshot" && event.data["view"]["terminal"]==true {registry.note_terminal_problem(&event);continue;}
                // `claude_wake`: mensagem nova para um processo morto; depois do teto, abre outra rodada.
                if event.channel=="claude_exit" || event.channel=="claude_wake" {
                    registry.schedule_claude_respawn(&event.key,event.generation).await;
                    continue;
                }
                if event.channel!="reset" {continue;}
                let name=registry.entries.lock().await.get(&event.key).filter(|e|e.generation==event.generation && e.provider=="claude" && matches!(e.handle,EntryHandle::Headless(_))).map(|e|e.name.clone());
                let Some(name)=name else {continue;};
                let current=match registry.interaction_binding_readonly(&name).await {Ok(current)=>current,Err(error)=>{
                    // Sem o vínculo a conversa nova não é religada: o chat fica na antiga e isso tem de aparecer.
                    tracing::warn!(session=%name,code=%error.code,reason=%error.message,"vínculo ilegível no reset; conversa não religada");
                    // Erro já na tela (o próprio motivo da leitura falhar) fica: não troca pela consequência.
                    let live=registry.live_sender(&name);
                    if live.borrow().as_ref().is_none_or(|state|state.error.is_none()) {super::actor::mark_live_error(&live,&error.code,&error.message);}
                    continue;
                }};
                if current.key!=event.key || current.generation!=event.generation || event.data["conversation"]!=current.conversation {continue;}
                if let (Some(hubs),Some(info))=(registry.hubs.get().and_then(crate::side::WeakHubs::upgrade),registry.claude_info(&name).await) {
                    if let Err(error)=hubs.rebind_claude_info(&name,info) {super::actor::mark_live_error(&registry.live_sender(&name),&error.code,&error.message);}
                }
            }
        });
    }
    /// Os mesmos códigos do `runtime_problem` do Python: erro do ator, ou a entrada parada e o motivo.
    fn note_terminal_problem(&self,event:&RuntimeEvent) {
        let stalled=|code:&str| match code {
            "composer_busy"=>"terminal_input_composer_busy","composer_unreadable"=>"terminal_input_composer_unreadable",
            "capture_failed"|"capture_utf8"=>"terminal_input_capture_failed","clear_not_applied"=>"terminal_clear_not_applied",
            "no_safe_delivery"=>"terminal_input_no_safe_delivery",
            _=>"terminal_input_stalled",
        };
        let input=event.data["view"]["input_stalled"].as_str().filter(|code|!code.is_empty());
        // Sem prova da conversa, a lista mostra a causa, não o erro que ela mesma causa na fila.
        let explained=input==Some("no_safe_delivery") && event.data["error"].as_str().is_some_and(|error|crate::state::edges::explained(input,error));
        let problem=if event.data["error"].is_string() && !explained {Some("runtime_falhou")} else {input.map(stalled)};
        let mut map=self.terminal_problems.lock().unwrap_or_else(|e|e.into_inner());
        match problem {
            Some(code)=>{map.insert(event.key.clone(),(event.generation,code.into()));},
            None=>{map.remove(&event.key);},
        }
    }
    /// Nome → problema dos atores com terminal abertos aqui, do último retrato publicado.
    pub async fn terminal_problems(&self)->BTreeMap<String,String> {
        let problems=self.terminal_problems.lock().unwrap_or_else(|e|e.into_inner()).clone();
        self.entries.lock().await.iter().filter(|(_,entry)|matches!(entry.handle,EntryHandle::Terminal {..}))
            .filter_map(|(key,entry)|problems.get(key).filter(|(generation,_)|*generation==entry.generation).map(|(_,code)|(entry.name.clone(),code.clone())))
            .collect()
    }
    pub fn bind_lifecycle(self:&Arc<Self>) {let _=self.self_ref.set(Arc::downgrade(self));}
    /// O recado nativo da operação `operation` da sessão `key`: seu id, já à espera de recibo, e o
    /// `from`, a caixa do Rust (ligada no primeiro recado). Cada recibo volta a essa operação.
    pub(crate) fn native_route(&self,key:&str,operation:&str) -> (String,String) {
        let message_id=crate::terminal_facts::message_id(key,operation);
        self.native.expect(&message_id,key,operation);
        let registry=self.self_ref.get().cloned();
        let origin=self.native.origin(move |receipt| {
            match registry.as_ref().and_then(std::sync::Weak::upgrade) {
                Some(registry)=>{tokio::spawn(async move {registry.native_receipt(receipt).await;});},
                None=>tracing::warn!(key=%receipt.key,code="native_receipt_unbound","recibo de recado nativo sem o registro do servidor"),
            }
        });
        (message_id,origin)
    }
    /// O `native_receipt` do coordenador do Python: retido ou recusado encerra a operação do recado com
    /// esse estado; entregue ou liberado a confirma.
    async fn native_receipt(&self,receipt:super::native_message::Receipt) {
        let super::native_message::Receipt {key,operation,state}=receipt;
        let (status,disposition)=match state.as_str() {
            "delivered"|"released"|""=>(Status::Accepted,"accepted"),"rejected"|"refused"=>(Status::Rejected,"rejected"),_=>(Status::Unknown,"unknown"),
        };
        let Some(handle)=self.entries.lock().await.get(&key).map(|entry|entry.handle.clone()) else {
            tracing::warn!(key=%key,state=%state,code="native_receipt_session_gone","recibo de recado nativo de sessão que já não está aberta");
            return;
        };
        let result=json!({"operation_id":operation,"disposition":disposition,"payload":{"native_status":state}});
        let applied=handle.queue(format!("native-receipt:{key}:{operation}:{state}"),Action::Finish {id:operation.clone(),status,result}).await;
        let applied=match applied {Ok(_) if status==Status::Accepted=>handle.confirm().await.map(|_|()),other=>other.map(|_|())};
        if let Err(error)=applied {
            tracing::warn!(key=%key,state=%state,code=%error.code,"recibo de recado nativo não aplicado");
        } else if status!=Status::Accepted {
            tracing::warn!(key=%key,state=%state,code="native_message_not_delivered","recado nativo retido ou recusado pelo receptor");
        }
    }
    async fn cancel_lifecycle_name(&self,name:&str) {
        for barrier in self.lifecycle.lock().await.values() {
            if let Ok(operation)=barrier.operation.lock() && let Some((owner,cancel))=operation.as_ref() && owner==name {cancel.store(true,std::sync::atomic::Ordering::Release);}
        }
    }
    /// Sem o sidecar a sessão foi fechada pelo nome: a entrada sai sem matar nada, em vez de religar e pôr
    /// erro numa sessão que não existe mais. Chamar com a barreira da chave; `absent`: ausência já lida nesta volta.
    async fn release_closed_by_name(&self,key:&str,generation:u64,name:&str,absent:bool) -> bool {
        if !absent && !super::lifecycle::claude_sidecar_absent(name).await {return false;}
        if let Err(error)=self.close_locked(key,generation,false).await {
            tracing::warn!(key,session=name,code=%error.code,"entrada da sessão fechada pelo nome não saiu do Rust");
        }
        true
    }
    async fn schedule_claude_respawn(self:&Arc<Self>,key:&str,generation:u64) {
        let handle=self.entries.lock().await.get(key).filter(|entry|entry.generation==generation && entry.provider=="claude").map(|entry|entry.handle.clone());
        let Some(handle)=handle else {return};
        let view=match handle.view().await {Ok(view)=>view,Err(error)=>{tracing::warn!(key,code=%error.code,"contexto da reabertura automática indisponível");return;}};
        let admitted={
            let mut entries=self.entries.lock().await;
            let Some(entry)=entries.get_mut(key).filter(|e|e.generation==generation && e.provider=="claude") else {return};
            if entry.respawn_inflight {return;}
            let Some(base)=entry.auto_respawn else {return};
            let (Some(target),Some(recipe),Some(birth))=(entry.runtime_target.clone(),entry.relaunch.clone(),entry.cano_birth) else {return};
            let Some(live)=entry.live.borrow().clone() else {return};
            let Some(binding)=live.binding.clone().filter(|b|b.key==key && b.generation==generation) else {return};
            entry.respawn_inflight=true;
            let cancelled=Arc::new(std::sync::atomic::AtomicBool::new(false));
            Some((entry.name.clone(),base,Arc::new(super::lifecycle::HeadlessOperation {target,binding,recipe,view,birth,auto:Some(base),cancelled})))
        };
        let Some((name,base,context))=admitted else {return};
        let barrier=self.barrier(key).await;
        if let Ok(mut op)=barrier.operation.lock() {*op=Some((name.clone(),context.cancelled.clone()));}
        let (registry,key)=(self.clone(),key.to_owned());
        tokio::spawn(async move {
            let id=format!("auto-restart:{key}:{generation}");
            let (mut gave_up,mut released)=(false,false);
            for attempt in 0..3 {
                // Já fechada pelo nome: a primeira volta a solta sem esperar.
                let absent=attempt==0 && super::lifecycle::claude_sidecar_absent(&name).await;
                if !absent {tokio::time::sleep(base * (1u32<<attempt)).await;}
                if context.cancelled.load(std::sync::atomic::Ordering::Acquire) {break;}
                let result=async {
                    let _ingress=super::lifecycle::ClosedIngress::acquire(&registry.ingress,&name).await?;
                    let _guard=barrier.lock().await;
                    // Sem a entrada desta vida, só segue quem a tirou numa volta anterior: outro a fechou depois
                    // de a religação ser aceita (o cancelamento do `close` pode ter vindo antes dela).
                    if !released && !registry.entries.lock().await.get(&key).is_some_and(|entry|entry.generation==generation) {return Err(failure("lifecycle_cancelled"));}
                    if registry.release_closed_by_name(&key,generation,&name,absent).await {return Err(failure(SESSION_CLOSED));}
                    let result=registry.reopen_headless_locked(&name,&key,generation,&id,None,true,Some(context.clone()),attempt).await;
                    released=!registry.entries.lock().await.contains_key(&key);
                    result
                }.await;
                match result {
                    Ok(_)=>break,
                    Err(error) if matches!(error.code.as_str(),"lifecycle_cancelled"|SESSION_CLOSED)=> {
                        tracing::info!(key,session=name,code=%error.code,"sessão fechada; reabertura automática cancelada");
                        break;
                    },
                    Err(error)=> {
                        super::actor::mark_live_error(&registry.live_sender(&name),&error.code,&error.message);
                        tracing::warn!(key,session=name,attempt,code=%error.code,"reabertura automática não confirmada");
                        if matches!(error.code.as_str(),"runtime_generation"|"runtime_binding"|"lifecycle_conversation") {break;}
                        gave_up=attempt==2 || super::claude_launch::permanent(&error.code);
                        if gave_up {break;}
                    },
                }
            }
            // Teto de subidas: o que esperava na fila vira "não chegou" em vez de sair horas depois, calado.
            if gave_up && !context.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                let handle=registry.entries.lock().await.get(&key).filter(|e|e.generation==generation).and_then(|e|match &e.handle {EntryHandle::Headless(h)=>Some(h.clone()),_=>None});
                if let Some(handle)=handle && let Err(error)=super::actor::abandon_queued(&handle).await {
                    tracing::warn!(key,session=name,code=%error.code,"fila não marcada como não chegou depois do teto de subidas");
                }
            }
            if let Ok(mut operation)=barrier.operation.lock() && operation.as_ref().is_some_and(|(_,current)|Arc::ptr_eq(current,&context.cancelled)) {*operation=None;}
            if let Some(entry)=registry.entries.lock().await.get_mut(&key).filter(|e|e.generation==generation) {entry.respawn_inflight=false;}
        });
    }
    /// O ator do Codex com terminal desta sessão está aberto aqui: o Rust publica o estado dela.
    pub fn owns_codex_terminal(&self,name:&str) -> bool { self.codex_owned.lock().unwrap_or_else(|e|e.into_inner()).contains_key(name) }
    /// Posse pela entrada (chave e geração), não só pelo nome: fechar uma vida antiga não desmarca a viva de mesmo nome.
    fn mark_codex_owner(&self,name:&str,key:&str,generation:u64,owned:bool) {
        let changed = {
            let mut map = self.codex_owned.lock().unwrap_or_else(|e|e.into_inner());
            let life = (key.to_owned(),generation);
            if owned { map.insert(name.to_owned(),life); true }
            else if map.get(name) == Some(&life) { map.remove(name); true }
            else { false }
        };
        if !changed { return; }
        if let Some(hubs) = self.hubs.get().and_then(crate::side::WeakHubs::upgrade) { hubs.set_runtime_owned(name,owned); }
    }
    pub fn with_interactions(mut self,store:Arc<crate::interactions::InteractionStore>) -> Self { self.interactions = Some(store); self }

    pub async fn interaction_binding(&self,name:&str) -> Result<crate::interactions::InteractionBinding,RuntimeError> {
        use crate::interactions::{InteractionBinding,InteractionError,runtime_error};
        let unavailable = ||runtime_error(InteractionError::Unavailable);
        let (key,generation,handle) = {
            let entries = self.entries.lock().await;
            let mut matches = entries.iter().filter(|(_,entry)|entry.name == name);
            let (key,entry) = matches.next().ok_or_else(unavailable)?;
            if matches.next().is_some() { return Err(unavailable()); }
            if let EntryHandle::Terminal { target,.. } = &entry.handle {
                if target.key != *key || target.generation != entry.generation || target.binding.conversation.is_empty() { return Err(unavailable()); }
                return Ok(InteractionBinding { key:key.clone(),conversation:target.binding.conversation.clone(),generation:entry.generation });
            }
            (key.clone(),entry.generation,entry.handle.clone())
        };
        let snapshot = handle.snapshot().await.map_err(|_|unavailable())?;
        let entries = self.entries.lock().await;
        if entries.iter().any(|(other,entry)|other != &key && entry.name == name) { return Err(unavailable()); }
        let entry = entries.get(&key).filter(|entry|entry.name == name && entry.generation == generation).ok_or_else(unavailable)?;
        if matches!(entry.handle,EntryHandle::Terminal {..}) || snapshot["key"] != key || snapshot["generation"] != generation || !snapshot["error"].is_null() { return Err(unavailable()); }
        // Logo depois de abrir a vista durável ainda não tem a conversa; o canal ao vivo, semeado pelo ator
        // antes da primeira resposta, tem o vínculo da mesma vida.
        let conversation = match snapshot["view"]["conversation"].as_str().filter(|s|!s.is_empty()) {
            Some(conversation) => conversation.to_owned(),
            None => entry.live.borrow().clone().and_then(|live|live.binding.clone())
                .filter(|binding|binding.key == key && binding.generation == generation && !binding.conversation.is_empty())
                .map(|binding|binding.conversation).ok_or_else(unavailable)?,
        };
        Ok(InteractionBinding { key,conversation,generation })
    }

    #[doc(hidden)]
    pub fn with_processes(mut self,processes:Arc<dyn crate::list::procs::ProcessView>)->Self {self.processes=processes;self}
    pub fn with_permissions(mut self,store:Arc<std::sync::Mutex<crate::state::permission::PermissionMemory>>)->Self {self.permissions=Some(store);self}
    pub fn with_respawn_base(mut self,base:Duration) -> Self { self.respawn_base = base; self }
    pub fn with_reconnect_waits(mut self,normal:Vec<Duration>,pending:Vec<Duration>,stall:Duration) -> Self {
        self.reconnect_waits = super::cano_reconnect::ReconnectWaits { normal,pending,stall,..self.reconnect_waits }; self
    }
    /// Só testes: o silêncio do cano que dá a saída dele como perdida (`OUTPUT_LOST`).
    #[doc(hidden)]
    pub fn with_output_lost(mut self,after:Duration) -> Self { self.reconnect_waits.output_lost = after; self }
    /// Só testes: a caixa dos recados nativos numa pasta própria, fora do `cc-socks` da máquina.
    #[doc(hidden)]
    pub fn with_native_inbox_dir(mut self,dir:std::path::PathBuf) -> Self { self.native=super::native_message::Inbox::in_dir(dir); self }
    /// Só testes: o `codex` falso de cada um vem do PATH deste ambiente, não do processo.
    #[doc(hidden)]
    pub fn with_codex_environment(mut self,env:crate::accounts::environment::AccountEnvironment) -> Self { self.policy = self.policy.with_codex_environment(env); self }
    /// Interface dos mods: sessão Claude sem terminal aberta aqui vira superfície remota e publica no `Mods`.
    pub fn with_mods(mut self,mods:crate::mods::state::Mods) -> Self { self.mods = Some(mods); self }
    /// A linha de status do ator lê a cota direto do módulo de contas.
    pub fn with_accounts(mut self,accounts:crate::accounts::AccountService) -> Self { self.policy = self.policy.with_accounts(accounts); self }
    /// A ponte do plugin que o Claude sem terminal subido aqui recebe no ambiente.
    pub fn with_plugin_bridge(mut self,bridge:crate::session_open::claude::PluginBridge) -> Self { self.policy = self.policy.with_plugin_bridge(bridge); self }
    pub fn with_plugin(mut self, plugin:Arc<crate::plugin::Store>) -> Self { self.plugin = Some(plugin); self }
    pub fn with_terminal_states(mut self, states:Arc<crate::state::published::Published>) -> Self { self.terminal_states=Some(states);self }
    pub fn with_terms(mut self, terms:Arc<crate::term::Terms>) -> Self {self.terms=Some(terms);self}
    fn terminal_mode_reader(&self) -> crate::terminal_facts::ModeReader {
        let (entries,published,permissions)=(Arc::downgrade(&self.entries),self.terminal_states.clone(),self.permissions.clone());
        Arc::new(move |name| {
            let (entries,published,permissions)=(entries.clone(),published.clone(),permissions.clone());
            Box::pin(async move {
                let entries=entries.upgrade().ok_or(crate::terminal_input::ServiceError("terminal_mode_unavailable"))?;
                let (mut source,terminal,live)={
                    let entries=entries.lock().await;
                    let mut matches=entries.iter().filter(|(_,entry)|entry.name==name && entry.provider=="claude");
                    let Some((key,entry))=matches.next() else {return Ok(None)};
                    if matches.next().is_some() {return Err(crate::terminal_input::ServiceError("terminal_mode_ambiguous"));}
                    if entry.mode_source.binding.key!=*key || entry.mode_source.binding.generation!=entry.generation || entry.mode_source.binding.conversation.is_empty() {
                        return Err(crate::terminal_input::ServiceError("terminal_mode_binding_changed"));
                    }
                    (entry.mode_source.clone(),matches!(&entry.handle,EntryHandle::Terminal {..}),entry.live.clone())
                };
                let declared=source.clone();
                if !terminal {
                    let current=live.borrow().clone().filter(|state|state.error.is_none()).ok_or(crate::terminal_input::ServiceError("terminal_mode_unavailable"))?;
                    let binding=current.binding.clone().filter(|binding|binding.key==source.binding.key && binding.generation==source.binding.generation && !binding.conversation.is_empty()).ok_or(crate::terminal_input::ServiceError("terminal_mode_binding_changed"))?;
                    source.transcript=source.transcript.with_file_name(format!("{}.jsonl",binding.conversation));
                    source.binding=binding;
                }
                let confirmed=match &permissions {
                    Some(memory)=>memory.lock().map_err(|_|crate::terminal_input::ServiceError("terminal_mode_unavailable"))?
                        .confirmed(&source.binding.conversation).map_err(crate::terminal_input::ServiceError)?,
                    None=>return Err(crate::terminal_input::ServiceError("terminal_mode_unavailable")),
                };
                // Base suposta (primeira leitura já em `plan`) não decide: vale o transcript ou o modo de abertura,
                // como o `session_non_plan_mode` do Python.
                let known=match &permissions {Some(memory)=>memory.lock().map_err(|_|crate::terminal_input::ServiceError("terminal_mode_unavailable"))?.base_known(&source.binding.conversation),None=>true};
                match confirmed.clone() {
                    Some(mode) if mode.current=="plan" && !known=>{},
                    Some(mode)=>source.confirmed=Some(if mode.current=="plan" {mode.previous_non_plan} else {mode.current}),
                    None if terminal=>if let Some(state)=published.as_ref().and_then(|published|published.get(&name,Some(&source.binding.conversation))) {
                        if state.problema.is_some() {return Err(crate::terminal_input::ServiceError("terminal_mode_unavailable"));}
                        source.confirmed=state.claude_permission_mode.clone().filter(|mode|!mode.is_empty() && mode!="plan")
                            .or_else(||state.claude_previous_non_plan.clone().filter(|mode|!mode.is_empty() && mode!="plan"));
                    },
                    None=>{
                        let current=live.borrow().clone().filter(|state|state.error.is_none()).ok_or(crate::terminal_input::ServiceError("terminal_mode_unavailable"))?;
                        let binding=current.binding.as_ref().filter(|binding|binding.key==source.binding.key && binding.generation==source.binding.generation && !binding.conversation.is_empty()).ok_or(crate::terminal_input::ServiceError("terminal_mode_binding_changed"))?;
                        if binding.conversation!=source.binding.conversation {return Err(crate::terminal_input::ServiceError("terminal_mode_binding_changed"));}
                        source.confirmed=current.public_state["claude_permission_mode"].as_str().filter(|mode|!mode.is_empty() && *mode!="plan")
                            .or_else(||current.public_state["claude_previous_non_plan"].as_str().filter(|mode|!mode.is_empty() && *mode!="plan"))
                            .map(str::to_owned).or(source.confirmed);
                    },
                }
                let entries=entries.lock().await;
                if !entries.get(&source.binding.key).is_some_and(|entry|entry.name==name && entry.generation==source.binding.generation && entry.mode_source.binding==declared.binding && entry.mode_source.transcript==declared.transcript) {
                    return Err(crate::terminal_input::ServiceError("terminal_mode_binding_changed"));
                }
                if !terminal && live.borrow().as_ref().and_then(|state|state.binding.as_ref())!=Some(&source.binding) {return Err(crate::terminal_input::ServiceError("terminal_mode_binding_changed"));}
                let fresh=permissions.as_ref().ok_or(crate::terminal_input::ServiceError("terminal_mode_unavailable"))?
                    .lock().map_err(|_|crate::terminal_input::ServiceError("terminal_mode_unavailable"))?
                    .confirmed(&source.binding.conversation).map_err(crate::terminal_input::ServiceError)?;
                if fresh!=confirmed {return Err(crate::terminal_input::ServiceError("terminal_mode_binding_changed"));}
                Ok(Some(source))
            })
        })
    }
    /// Classe do recado (`bypass`/`prompting`) para o Python, que não guarda mais a memória de modo: a do
    /// remetente quando é sessão aberta aqui, senão a do alvo. `None`: nenhum modo confirmado; o Python cai
    /// no transcript e na conta, como antes.
    pub(crate) async fn message_class(&self,sender:Option<&str>,target:&str) -> Result<&'static str,RuntimeError> {
        let read=self.terminal_mode_reader();
        let class=|mode:&str| if mode.to_ascii_lowercase().contains("bypass") {"bypass"} else {"prompting"};
        // Leitura que falha é erro com código, nunca o Python recalculando com o Rust de pé.
        if let Some(sender)=sender && let Some(source)=read(sender.to_owned()).await.map_err(|e|failure(e.0))?
            && let Some(mode)=source.confirmed {return Ok(class(&mode));}
        let source=read(target.to_owned()).await.map_err(|e|failure(e.0))?.ok_or_else(||failure("runtime_binding"))?;
        if let Some(mode)=source.confirmed {return Ok(class(&mode));}
        let (agent,config)=self.launch_facts_of(target).await.ok_or_else(||failure("terminal_account_mode_unavailable"))?;
        let transcript=source.transcript.clone();
        let mode=tokio::task::spawn_blocking(move||crate::terminal_facts::unconfirmed_mode(&transcript,agent,&config)).await
            .map_err(|_|failure("terminal_mode_unavailable"))?.map_err(|e|failure(e.0))?;
        Ok(class(&mode))
    }
    /// Agente com terminal (o argv dele traz o modo de abertura) e pasta da conta da sessão aberta aqui (sem
    /// a gravada, a padrão do Claude). Sem terminal o modo de abertura já vem confirmado do sidecar.
    async fn launch_facts_of(&self,name:&str) -> Option<(Option<u32>,std::path::PathBuf)> {
        let entries=self.entries.lock().await;
        let entry=entries.values().find(|entry|entry.name==name)?;
        match &entry.handle {
            EntryHandle::Terminal {target,..}=>Some((Some(target.metadata.agent_pid),target.metadata.config_dir.clone())),
            EntryHandle::Headless(_)=>entry.runtime_target.as_ref().and_then(|target|target.metadata["config_dir"].as_str().map(std::path::PathBuf::from))
                .or_else(||std::env::home_dir().map(|home|home.join(".claude"))).map(|config|(None,config)),
            EntryHandle::CodexTerminal {..}=>None,
        }
    }
    /// Vínculo e metadados do terminal, sem enviar um pedido ao ator ocupado.
    pub async fn terminal_target(&self,name:&str)->Result<Option<super::terminal::TerminalTarget>,RuntimeError> {
        let entries=self.entries.lock().await;let mut matches=entries.iter().filter(|(_,entry)|entry.name==name);
        let Some((key,entry))=matches.next() else {return Ok(None)};
        if matches.next().is_some() {return Err(failure("runtime_binding"));}
        match &entry.handle {
            EntryHandle::Terminal {target,..} if target.key==*key && target.generation==entry.generation && target.binding.generation==entry.generation && target.binding.name==name=>Ok(Some(target.clone())),
            EntryHandle::Terminal {..}=>Err(failure("runtime_binding")),_=>Ok(None),
        }
    }
    pub async fn interaction_binding_readonly(&self,name:&str)->Result<crate::interactions::InteractionBinding,RuntimeError> {
        let entries=self.entries.lock().await;let mut matches=entries.iter().filter(|(_,entry)|entry.name==name);
        let (key,entry)=matches.next().ok_or_else(||failure("runtime_binding"))?;
        if matches.next().is_some() {return Err(failure("runtime_binding"));}
        if let EntryHandle::Terminal {target,..}=&entry.handle {
            if target.key!=*key || target.generation!=entry.generation || target.binding.generation!=entry.generation || target.binding.name!=name || target.binding.conversation.is_empty() {return Err(failure("runtime_binding"));}
            return Ok(crate::terminal_facts::LocalTerminalFacts::binding(target));
        }
        let live=entry.live.borrow().clone().filter(|state|state.error.is_none()).ok_or_else(||failure("runtime_binding"))?;
        let binding=live.binding.clone().filter(|binding|binding.key==*key && binding.generation==entry.generation && !binding.conversation.is_empty()).ok_or_else(||failure("runtime_binding"))?;
        Ok(binding)
    }
    /// Transcript da conversa vigente do Claude, conferido sob a mesma trava do vínculo: a medida de
    /// contexto do plugin nunca cai no arquivo de outra vida nem no metadado do boot.
    pub async fn context_transcript(&self,name:&str)->Result<(crate::interactions::InteractionBinding,std::path::PathBuf),RuntimeError> {
        let entries=self.entries.lock().await;let mut matches=entries.iter().filter(|(_,entry)|entry.name==name);
        let (key,entry)=matches.next().ok_or_else(||failure("runtime_binding"))?;
        if matches.next().is_some() {return Err(failure("runtime_binding"));}
        if entry.provider!="claude" {return Err(failure("runtime_provider"));}
        match &entry.handle {
            EntryHandle::Terminal {target,..}=> {
                if target.key!=*key || target.generation!=entry.generation || target.binding.generation!=entry.generation || target.binding.name!=name || target.binding.conversation.is_empty() {return Err(failure("runtime_binding"));}
                Ok((crate::terminal_facts::LocalTerminalFacts::binding(target),target.transcript.clone()))
            },
            EntryHandle::Headless(_)=> {
                let live=entry.live.borrow().clone().filter(|state|state.error.is_none()).ok_or_else(||failure("runtime_binding"))?;
                let binding=live.binding.clone().filter(|binding|binding.key==*key && binding.generation==entry.generation && !binding.conversation.is_empty() && entry.mode_source.binding.key==*key).ok_or_else(||failure("runtime_binding"))?;
                let transcript=entry.mode_source.transcript.with_file_name(format!("{}.jsonl",binding.conversation));
                Ok((binding,transcript))
            },
            EntryHandle::CodexTerminal {..}=>Err(failure("runtime_provider")),
        }
    }
    /// Por que a sessão Claude ficou parada no limite, no estado ao vivo dela. Sem terminal o ator o leva até
    /// o próximo turno bom; com terminal ele fica no canal de estado até a próxima entrega ou `None`.
    pub async fn show_limit_problem(&self,name:&str,problem:Option<(String,String)>) {
        let handle={
            let entries=self.entries.lock().await;let mut matches=entries.values().filter(|entry|entry.name==name&&entry.provider=="claude");
            match (matches.next(),matches.next()) {(Some(entry),None)=>entry.handle.clone(),_=>return}
        };
        match handle {
            EntryHandle::Headless(handle)=>if let Some((code,detail))=problem && let Err(error)=handle.show_problem(&code,Some(detail)).await {
                tracing::warn!(session=%name,code=%error.code,"o problema do limite não chegou ao estado da sessão");
            },
            EntryHandle::Terminal {..}=>super::terminal_status::publish_limit(&self.terminal_status_sender(name),problem),
            EntryHandle::CodexTerminal {..}=>{},
        }
    }
    /// Uma ação na fila durável da sessão viva `name`, pelo ator dela.
    pub async fn queue_action(&self,name:&str,id:String,action:Action)->Result<Value,RuntimeError> {
        let handle={
            let entries=self.entries.lock().await;let mut matches=entries.values().filter(|entry|entry.name==name);
            let entry=matches.next().ok_or_else(||failure("runtime_binding"))?;
            if matches.next().is_some() {return Err(failure("runtime_binding"));}entry.handle.clone()
        };handle.queue(id,action).await
    }
    /// O Claude com terminal sem ator (restaurado depois da adoção da subida) é adotado como na escrita,
    /// pela porta. O sem terminal estacionado não sobe por isto.
    pub async fn adopt_claude_terminal(&self,name:&str)->Result<(),RuntimeError> {
        if self.has_entry(None,name).await {return Ok(());}
        let Some(watcher)=self.watcher.get().cloned() else {return Ok(())};
        let _pass=self.ingress.enter(name,Duration::ZERO).await
            .map_err(|_|RuntimeError::new("session_transfer_busy","a sessão está trocando de agente"))?;
        watcher.adopt_claude(self,name).await.unwrap_or(Ok(()))
    }
    /// O alvo do Claude com terminal para conferir o limite de uso no fim do turno; `None`: a sessão é sem
    /// terminal, e quem vê o limite é o ator dela.
    /// Sem alvo, o `limit_switch_no_target` aparece na linha (o canal do nome existe sem ator) até o alvo voltar.
    pub async fn limit_target(&self,name:&str)->Result<Option<super::terminal::TerminalTarget>,RuntimeError> {
        const NO_TARGET:&str="limit_switch_no_target";
        let found=match self.adopt_claude_terminal(name).await {
            Ok(())=>self.terminal_target(name).await,
            Err(error)=>Err(error),
        };
        let sender=self.terminal_status_sender(name);
        match found {
            Ok(Some(target))=>{
                if sender.borrow().limit.as_ref().is_some_and(|(code,_)|code==NO_TARGET) {super::terminal_status::publish_limit(&sender,None);}
                Ok(Some(target))
            }
            Ok(None) if self.has_entry(None,name).await=>Ok(None),
            other=>{
                let cause=other.err().map_or_else(||NO_TARGET.to_owned(),|error|error.code);
                super::terminal_status::publish_limit(&sender,Some((NO_TARGET.to_owned(),cause)));
                Err(failure(NO_TARGET))
            }
        }
    }
    /// O fim de turno do Claude: o aviso de limite no fim do transcript da conversa vigente vai ao vigia.
    pub async fn turn_limit(&self,name:&str,binding:&crate::interactions::InteractionBinding,now:f64)->Result<TurnLimit,RuntimeError> {
        let Some(target)=self.limit_target(name).await? else {return Ok(TurnLimit::NotTerminal)};
        let (live,transcript)=self.context_transcript(name).await?;
        if live!=*binding {return Err(failure("plugin_conversation_mismatch"));}
        let path=transcript.clone();
        let limit=tokio::task::spawn_blocking(move||crate::session_write::limit_switch::file_limit(&path,now)).await
            .map_err(|_|failure("limit_transcript_read_job"))?;
        let sent=limit.clone().is_some_and(|limit|self.usage_limit(LimitHit { name:name.to_owned(),provider:"claude".into(),
            account:Some(target.metadata.config_dir.to_string_lossy().into_owned()),limit }));
        Ok(TurnLimit::Checked { transcript,limit,sent })
    }
    pub async fn drain(&self,name:&str)->Result<Value,RuntimeError> {
        self.adopt_claude_terminal(name).await?;
        let handle={
            let entries=self.entries.lock().await;let mut matches=entries.values().filter(|entry|entry.name==name);
            let entry=matches.next().ok_or_else(||failure("runtime_binding"))?;
            if matches.next().is_some() {return Err(failure("runtime_binding"));}entry.handle.clone()
        };handle.drain().await
    }
    pub fn plugin(&self) -> Result<&Arc<crate::plugin::Store>,RuntimeError> {
        self.plugin.as_ref().ok_or_else(||failure("plugin_store_unavailable"))
    }
    async fn bind_plugin(&self,name:&str) -> Result<(),RuntimeError> {
        if let Some(plugin) = &self.plugin {
            let binding = self.interaction_binding(name).await?;
            plugin.bind(&binding).map_err(|error|failure(error.0))?;
        }
        Ok(())
    }
    pub fn ingress(&self) -> &super::ingress::IngressGates { &self.ingress }
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> { self.events.subscribe() }
    /// Último valor da sessão `name` para o feed do hub; `None` enquanto ela não está aberta aqui.
    pub fn live(&self,name:&str) -> LiveReceiver {
        let mut map = self.live.lock().unwrap_or_else(|e|e.into_inner());
        map.retain(|key,sender|key == name || sender.receiver_count() > 0 || sender.sender_count() > 1 || sender.borrow().is_some());
        map.entry(name.to_owned()).or_insert_with(||tokio::sync::watch::channel(None).0).subscribe()
    }
    /// Estado do envio terminal, sem receber eventos das outras sessões.
    pub fn terminal_status(&self,name:&str) -> super::terminal_status::StatusReceiver {
        // Assinar sob a trava evita que a limpeza confunda o leitor com um publicador novo.
        let mut map=self.terminal_status.lock().unwrap_or_else(|e|e.into_inner());
        map.retain(|key,sender|key==name || sender.receiver_count()>0 || sender.sender_count()>1 || sender.borrow().error.is_some());
        map.entry(name.to_owned()).or_insert_with(||tokio::sync::watch::channel(Default::default()).0).subscribe()
    }
    pub(crate) fn terminal_status_sender(&self,name:&str) -> super::terminal_status::StatusSender {
        let mut map=self.terminal_status.lock().unwrap_or_else(|e|e.into_inner());
        map.retain(|key,sender|key==name || sender.receiver_count()>0 || sender.sender_count()>1 || sender.borrow().error.is_some());
        map.entry(name.to_owned()).or_insert_with(||tokio::sync::watch::channel(Default::default()).0).clone()
    }
    /// Canal do nome, criado na primeira procura (feed ou ator, o que vier antes). Canal vazio e sem
    /// receptor sai do mapa aqui: sem isto o mapa guardaria todo nome que já teve chat aberto. Canal
    /// que um ator já segura (cópia fora do mapa) fica, mesmo vazio: o ator só escreve no primeiro passo.
    fn live_sender(&self,name:&str) -> LiveSender {
        let mut map = self.live.lock().unwrap_or_else(|e|e.into_inner());
        map.retain(|key,sender|key == name || sender.receiver_count() > 0 || sender.sender_count() > 1 || sender.borrow().is_some());
        map.entry(name.to_owned()).or_insert_with(||tokio::sync::watch::channel(None).0).clone()
    }
    pub async fn handle(&self,key:&str,generation:u64) -> Result<RuntimeHandle,RuntimeError> {
        match self.entry(key,generation).await? {EntryHandle::Headless(handle) | EntryHandle::CodexTerminal {handle,..}=>Ok(handle),_=>Err(failure("runtime_provider"))}
    }
    async fn entry(&self,key:&str,generation:u64)->Result<EntryHandle,RuntimeError> {
        self.entries.lock().await.get(key).filter(|entry|entry.generation==generation)
            .map(|entry|entry.handle.clone()).ok_or_else(||failure("runtime_binding"))
    }
    /// Abre a sessão no Rust: trava, fila com `Recover`, conexão ao cano e ator. Responde sem
    /// esperar o `initialize`; o ator o faz e drena a fila quando a sessão fica entregável.
    pub async fn open(&self,target:RuntimeTarget) -> Result<Value,RuntimeError> { self.open_inner(target,None,false).await }
    /// `open` que pode subir o processo: sobe só se o cano gravado não for `Ours` (vivo e da chave).
    pub async fn open_with_launch(&self,target:RuntimeTarget,sidecar_dir:std::path::PathBuf) -> Result<Value,RuntimeError> {
        self.open_inner(target,Some(sidecar_dir),true).await
    }
    /// `open` de sessão cujo processo o Rust administra (religar, reiniciar, encerrar) sem subir agora.
    pub async fn open_managed(&self,target:RuntimeTarget,sidecar_dir:std::path::PathBuf) -> Result<Value,RuntimeError> {
        self.open_inner(target,Some(sidecar_dir),false).await
    }
    /// Abertura que falha fica no canal do hub até a próxima abertura (a mesma regra do `close` com
    /// erro): sem isso o chat mostraria a sessão parada. Geração ou provedor errado é pedido torto,
    /// não a sessão falhando.
    async fn open_inner(&self,target:RuntimeTarget,managed:Option<std::path::PathBuf>,spawn:bool) -> Result<Value,RuntimeError> {
        let (name,key,claude) = (target.name.clone(),target.key.clone(),target.provider == "claude");
        let sidecars = managed.clone();
        let result = self.open_attempt(target,managed,spawn).await;
        if let Err(error) = &result && !["runtime_generation","runtime_provider"].contains(&error.code.as_str()) {
            super::actor::mark_live_error(&self.live_sender(&name),&error.code,&error.message);
            // A queda vai ao arquivo da sessão (a próxima vida a mostra); trava ou conta ocupada é passageira.
            if let (true,Some(dir)) = (claude,sidecars) && !["runtime_lease","account_busy"].contains(&error.code.as_str()) {
                let detail:String = format!("{}: {}",error.code,error.message).chars().take(300).collect();
                let patch = [("problema".to_owned(),json!(["headless_nao_subiu",detail]))].into_iter().collect();
                let written = tokio::task::spawn_blocking({let name=name.clone();move||super::codex_sidecar::update_if(&dir,&name,&[("key",Value::from(key))],&patch)}).await;
                if !matches!(written,Ok(Ok(_))) { tracing::warn!(session=%name,code=%error.code,"a falha de subida não foi gravada no arquivo da sessão"); }
            }
        }
        result
    }
    async fn open_attempt(&self,target:RuntimeTarget,managed:Option<std::path::PathBuf>,spawn:bool) -> Result<Value,RuntimeError> {
        self.cancel_lifecycle_name(&target.name).await;
        let barrier=self.barrier(&target.key).await;barrier.cancel();let _guard=barrier.lock().await;
        self.open_headless_locked(target,managed,spawn,true).await
    }
    /// `adopt_unreachable`: o cano gravado é um vivo de antes, e se ele não atender o ator nasce religando. Quem
    /// acabou de subir o cano passa `false`: a falha volta para ele desfazer a subida.
    async fn open_headless_locked(&self,mut target:RuntimeTarget,managed:Option<std::path::PathBuf>,spawn:bool,adopt_unreachable:bool)->Result<Value,RuntimeError> {
        let launch=managed.as_ref().filter(|_|spawn);
        let existing = self.entries.lock().await.get(&target.key).map(|entry|(entry.generation,entry.handle.clone()));
        let handle = if let Some((generation,handle)) = existing {
            if generation != target.generation { return Err(failure("runtime_generation")); }
            match handle {EntryHandle::Headless(handle)=>handle,_=>return Err(failure("runtime_provider"))}
        } else {
        if !["claude","codex"].contains(&target.provider.as_str()) || target.binding.versao != 2 { return Err(failure("runtime_provider")); }
        let account_guard = runtime_account_guard(&target.provider, &target.metadata).await?;
        let lease = wait_lease(&target.lease_path).await?;
        let store = open_store(&target.state_path,&target.projection_dir,&target.key,target.generation,&target.name,lease.clone()).await?;
        let mut metadata = target.metadata.clone();
        if let Some(fields) = store.state().runtime_state["view"].as_object() {
            if fields.get("conversation").is_some_and(|conversation|conversation == &metadata[if target.provider == "claude" { "session_id" } else { "thread_id" }]) {
                for (key,value) in fields { if key != "public_state" { metadata[key] = value.clone(); } }
            }
        }
        let queue = QueueActor::start(store,lease);
        // Depois da fila: o `session.patch_meta` do Python lê o estado dela.
        let launched = match &launch {
            Some(sidecar_dir)=>launch_if_needed(&self.policy,&mut target,sidecar_dir).await,
            None=>Ok(None),
        };
        let connection = match &launched {
            Err(error)=>Err(error.clone()),
            Ok(_)=>cano::connect(&target.binding).await,
        };
        // Cano vivo da chave (não o subido aqui) que não atendeu: o ator nasce religando a ele, nunca sem ator.
        let adopted_live = adopt_unreachable && connection.is_err() && target.provider == "claude" && matches!(launched,Ok(None)) && {
            let (pid,key) = (target.binding.pid,target.key.clone());
            tokio::task::spawn_blocking(move||process::liveness(pid,&key)==process::Liveness::Ours).await.unwrap_or(false)
        };
        let connection = match connection {
            Ok(connection)=>Some(connection),
            Err(error) if adopted_live=>{
                tracing::info!(key=%target.key,session=%target.name,code=%error.code,"cano vivo não atendeu a abertura; o ator nasce religando");
                None
            },
            Err(error)=>{
                if let Err(stop) = queue.shutdown().await {
                    tracing::warn!(key=%target.key,code=%error.code,stop=?stop.kind(),"fila não fechou depois da falha ao conectar no cano");
                }
                // Só o cano subido nesta chamada morre: um vivo de antes pode estar no meio de um turno.
                if let (Ok(Some(cano)),Some(sidecar_dir)) = (launched,launch) { discard(&target,&cano,sidecar_dir).await; }
                return Err(error);
            },
        };
        let fresh = matches!(launched,Ok(Some(_)));
        let epoch_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0);
        let revision = self.revisions.lock().await.entry(target.key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
        let mut engine = RuntimeEngine::new(&target.provider,metadata,target.generation,ClockSample { monotonic_s:0.0,epoch_s })?
            .with_policy(self.policy.clone()).with_publisher(self.events.clone()).with_revision(revision);
        if let Some(store) = &self.interactions { engine = engine.with_interactions(target.key.clone(),store.clone())?; }
        engine.set_fresh_process(fresh);
        let claude_recipe=if target.provider=="claude" {
            let procs=self.processes.clone();
            let sid=target.metadata["session_id"].as_str().ok_or_else(||failure("lifecycle_conversation"))?;
            // Em motor o filho do cano é o `hangar-engine` até o exec virar o `claude`, e o cano já escuta antes.
            let deadline=tokio::time::Instant::now()+Duration::from_secs(15);
            let agent=loop {
                match super::lifecycle::launched_agent(procs.as_ref(),i64::from(target.binding.pid),sid) {
                    Err(error) if error.code=="relaunch_agent_identity" && tokio::time::Instant::now()<deadline=>tokio::time::sleep(Duration::from_millis(50)).await,
                    found=>break found,
                }
            };
            let (pid,birth)=match agent {
                Ok(agent)=>agent,
                Err(error)=>{
                    if let Err(stop) = queue.shutdown().await {
                        tracing::warn!(key=%target.key,code=%error.code,stop=?stop.kind(),"fila não fechou depois de não achar o claude do cano");
                    }
                    if let (Ok(Some(cano)),Some(sidecar_dir)) = (launched,launch) { discard(&target,&cano,sidecar_dir).await; }
                    return Err(error);
                },
            };
            let process=procs.recipe(pid).map_err(|_|failure("lifecycle_process_read"))?;
            let argv_sid=super::claude_relaunch::Recipe::argument_conversation(&process)?;
            // Sem conexão não há retrato para conferir; a conversa que ele trouxer ao religar passa pela troca de conversa do ator.
            if argv_sid!=sid && connection.as_ref().is_some_and(|connection|!connection.snapshot.init.as_ref().and_then(|raw|serde_json::from_str::<Value>(raw).ok()).is_some_and(|event|event["session_id"]==sid)) {return Err(failure("relaunch_conversation"));}
            let mut recipe=super::claude_relaunch::Recipe::capture(procs.as_ref(),i64::from(target.binding.pid),pid,birth,&argv_sid,false)?;
            recipe.preserve_settings(target.state_path.parent().ok_or_else(||failure("relaunch_settings_copy"))?)?;
            Some(Arc::new(recipe))
        } else {None};
        let adopted_birth=claude_recipe.as_ref().map(|recipe|recipe.root_identity().map(|(_,birth)|birth)).transpose()?;
        if target.provider=="codex" && let Some(sidecar_dir) = &managed { engine = engine.with_launch(LaunchConfig { sidecar_dir:sidecar_dir.clone(),backoff:self.respawn_base }); }
        if let Some(mods) = &self.mods { engine = engine.with_mods(mods.clone()); }
        engine = engine.with_live(self.live_sender(&target.name)).with_lifecycle(self.self_ref.get().cloned())
            .with_cano_birth(adopted_birth).with_reconnect_waits(self.reconnect_waits.normal.clone(),self.reconnect_waits.pending.clone(),self.reconnect_waits.stall)
            .with_cano_check(self.reconnect_waits.check).with_output_lost(self.reconnect_waits.output_lost);
        // Dono único dos pedidos dos apps até o `close` (S9). Só o Claude tem superfície. Registrado antes
        // de a tarefa do ator existir: a primeira faixa publicada já encontra a sessão no `Mods`.
        // A vida no `Mods` é única no servidor; o processo é a chave durável mais o cano, que o renomear mantém.
        let life = engine.mods_life();
        let attach = |handle:&RuntimeHandle| {
            if target.provider == "claude" && let Some(mods) = &self.mods {
                let process = format!("{}:{}:{}",target.key,target.binding.pid,target.binding.escuta);
                mods.attach_process(&target.name,&process,life,Arc::new(handle.clone()));
            }
        };
        let handle = match connection {
            Some(connection)=>RuntimeActor::spawn_with(target.clone(),queue,connection,engine,attach),
            None=>RuntimeActor::spawn_unconnected_with(target.clone(),queue,engine,attach),
        };
        if let Ok(mut parked)=self.barrier(&target.key).await.parked.lock() {*parked=None;}
        self.entries.lock().await.insert(target.key.clone(),Entry { generation:target.generation,handle:EntryHandle::Headless(handle.clone()),
            lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:life,provider:target.provider.clone(),account_key:Some(account_guard.key.clone()),relaunch:claude_recipe,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding:crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:target.metadata[if target.provider=="claude" {"session_id"} else {"thread_id"}].as_str().unwrap_or("").into()},transcript:target.transcript.clone(),confirmed:target.metadata["permission_mode"].as_str().filter(|mode|*mode!="plan").map(str::to_owned)},
            live:self.live_sender(&target.name),runtime_target:(target.provider=="claude").then(||target.clone()),
            cano_birth:adopted_birth,auto_respawn:managed.as_ref().filter(|_|target.provider=="claude").map(|_|self.respawn_base),respawn_inflight:false });
        handle
        };
        let snapshot = match handle.snapshot().await {
            Ok(snapshot) if snapshot["error"].is_null() && snapshot["view"]["alive"] != false=>snapshot,
            result=>{
                // Ator morto ou cano já saído: fecha agora, senão a entrada presa responderia ao próximo `open`.
                let error = match result {
                    Err(error)=>error,
                    Ok(snapshot)=>snapshot["error"].as_str().map_or_else(||failure("cano_exited"),|code|RuntimeError::new(code,"ator do runtime terminou ao abrir")),
                };
                if let Err(close) = self.close_locked(&target.key,target.generation,false).await {
                    tracing::warn!(key=%target.key,code=%close.code,"sessão não fechou depois de abrir com erro");
                }
                return Err(error);
            }
        };
        if target.provider == "claude" { self.bind_plugin(&target.name).await?; }
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    /// Adota o app-server do Codex com terminal: confere endereço local, conversa e pid, abre a fila sob a
    /// trava e liga o ator ao WebSocket. Quem chama é a vigia (`codex_terminal::Watcher`); nada sobe aqui.
    pub async fn open_codex_terminal(&self,target:CodexTerminalTarget) -> Result<Value,RuntimeError> {
        let name = target.name.clone();
        let result = self.open_codex_terminal_attempt(target).await;
        if let Err(error) = &result && !["runtime_generation","runtime_provider"].contains(&error.code.as_str()) {
            super::actor::mark_live_error(&self.live_sender(&name),&error.code,&error.message);
        }
        result
    }
    async fn open_codex_terminal_attempt(&self,target:CodexTerminalTarget) -> Result<Value,RuntimeError> {
        let barrier = self.barrier(&target.key).await;
        let _guard = barrier.lock().await;
        let existing = self.entries.lock().await.get(&target.key).map(|entry|(entry.generation,entry.handle.clone()));
        let handle = match existing {
            Some((generation,EntryHandle::CodexTerminal { target:old,handle,.. })) if generation == target.generation && old.same_life(&target)=>{
                // Mesma vida, mas o app-server pode ter morrido: responder `opened` com um ator desconectado para
                // sempre esconderia isso. Quem fecha e adota a vida nova é a vigia.
                super::codex_terminal::verify(&target).await?;
                handle
            },
            Some(_)=>return Err(failure("runtime_generation")),
            None=>{
                super::codex_terminal::verify(&target).await?;
                let account_guard = runtime_account_guard("codex",&target.metadata).await?;
                let lease = wait_lease(&target.lease_path).await?;
                let store = open_store(&target.state_path,&target.projection_dir,&target.key,target.generation,&target.name,lease.clone()).await?;
                let runtime = target.runtime_target();
                let mut metadata = runtime.metadata.clone();
                if let Some(fields) = store.state().runtime_state["view"].as_object() {
                    if fields.get("conversation").is_some_and(|conversation|conversation == &metadata["thread_id"]) {
                        for (key,value) in fields { if key != "public_state" && key != "headless" { metadata[key] = value.clone(); } }
                    }
                }
                let queue = QueueActor::start(store,lease);
                let connection = match super::websocket::WebSocketConnection::connect(&target.endpoint).await {
                    Ok(connection)=>connection,
                    Err(error)=>{
                        if let Err(stop) = queue.shutdown().await {
                            tracing::warn!(key=%target.key,code=%error.code,stop=?stop.kind(),"fila não fechou depois da falha ao conectar no app-server");
                        }
                        return Err(error);
                    }
                };
                let epoch_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0);
                let revision = self.revisions.lock().await.entry(target.key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
                let mut engine = RuntimeEngine::new("codex",metadata,target.generation,ClockSample { monotonic_s:0.0,epoch_s })?
                    .with_policy(self.policy.clone()).with_publisher(self.events.clone()).with_revision(revision);
                if let Some(store) = &self.interactions { engine = engine.with_interactions(target.key.clone(),store.clone())?; }
                engine = engine.with_live(self.live_sender(&target.name)).with_lifecycle(self.self_ref.get().cloned());
                let cell = AdoptionCell::new(&target.endpoint,target.app_pid).with_lifecycle(self.codex_wake.clone(),target.sidecar_life());
                let adoption = Adoption { endpoint:target.endpoint.clone(),app_pid:target.app_pid,cell:cell.clone() };
                let handle = RuntimeActor::spawn_adopted(runtime,queue,connection,engine,adoption);
                self.entries.lock().await.insert(target.key.clone(),Entry { generation:target.generation,
                    handle:EntryHandle::CodexTerminal { target:Box::new(target.clone()),handle:handle.clone(),cell },
                    lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:0,provider:"codex".into(),account_key:Some(account_guard.key.clone()),relaunch:None,settings_owner:None,
                    mode_source:crate::terminal_facts::ModeSource {binding:crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:target.thread_id.clone()},transcript:target.transcript.clone().unwrap_or_default(),confirmed:None},
                    live:self.live_sender(&target.name),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false });
                // O estado passa a sair do feed do runtime; o pane segue presente.
                self.mark_codex_owner(&target.name,&target.key,target.generation,true);
                handle
            }
        };
        let snapshot = match handle.snapshot().await {
            Ok(snapshot) if snapshot["error"].is_null()=>snapshot,
            result=>{
                let error = match result {
                    Err(error)=>error,
                    Ok(snapshot)=>snapshot["error"].as_str().map_or_else(||failure("runtime_closed"),|code|RuntimeError::new(code,"ator do runtime terminou ao abrir")),
                };
                if let Err(close) = self.close_locked(&target.key,target.generation,false).await {
                    tracing::warn!(key=%target.key,code=%close.code,"sessão não fechou depois de abrir com erro");
                }
                return Err(error);
            }
        };
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    pub async fn open_terminal(&self,target:super::terminal::TerminalTarget)->Result<Value,RuntimeError> {
        // O mesmo pane com outra chave seriam dois atores escrevendo no mesmo terminal.
        let twin=self.entries.lock().await.values().any(|e|e.name==target.name && matches!(&e.handle,EntryHandle::Terminal {target:old,..}
            if old.key!=target.key && old.binding.pane==target.binding.pane && old.binding.created==target.binding.created));
        if twin {return Err(failure("runtime_binding"));}
        let barrier=self.barrier(&target.key).await; let _guard=barrier.lock().await;
        self.open_terminal_locked(target).await
    }
    async fn open_terminal_locked(&self,target:super::terminal::TerminalTarget)->Result<Value,RuntimeError> {
        let existing=self.entries.lock().await.get(&target.key).map(|e|(e.generation,e.handle.clone(),e.mods_life));
        let handle=match existing {
            // A mesma vida, descrita pelo Rust (adoção) ou pelo Python (texto que nasce lá): o pane, a
            // conversa, o agente e a fila são os mesmos; a impressão digital e o programa do multiplexador
            // podem ser escritos de outro jeito.
            Some((generation,EntryHandle::Terminal {target:old,handle},life)) if generation==target.generation && same_terminal_life(&old,&target)=>{
                // Reabertura da mesma vida: se o nome saiu do `Mods`, a sessão volta a ser ligada, na vida da
                // entrada, que é a que o `close` esquece. Nome com outra vida fica com ela: é uma sessão mais
                // nova e viva, e tomá-lo a deixaria sem dono e sem o nome de nascimento.
                if let Some(mods)=&self.mods && life!=0 && mods.life(&target.name).is_none() {
                    attach_terminal_mods(mods,&target,&handle,life).await;
                }
                handle
            },
            Some(_)=>return Err(failure("runtime_generation")),
            None=>{
                let lease=wait_lease(&target.lease_path).await?;
                let store=open_store(&target.state_path,&target.projection_dir,&target.key,target.generation,&target.name,lease.clone()).await?;
                let revision=self.revisions.lock().await.entry(target.key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
                if let Some(interactions) = &self.interactions {
                    interactions.bind(&crate::interactions::InteractionBinding { key:target.key.clone(),conversation:target.binding.conversation.clone(),generation:target.generation })
                        .map_err(crate::interactions::runtime_error)?;
                }
                // A âncora nasce com o executor, que a lê, e passa ao elo pelo `handle`, também na reabertura.
                let options=super::terminal::TerminalOptions {lifecycle:self.self_ref.get().cloned(),permissions:self.permissions.clone(),interactions:self.interactions.clone(),plugin:self.plugin.clone(),mode_facts:Some(self.terminal_mode_reader()),terms:self.terms.clone(),anchor:super::terminal::ModsAnchor::default(),status:Some(self.terminal_status_sender(&target.name)),..Default::default()};
                let handle=super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),self.policy.clone(),options,self.events.clone(),revision);
                // A vida no `Mods` é única no servidor (`new_life`).
                let life=match &self.mods {
                    Some(mods)=>{
                        let life=mods.new_life();
                        attach_terminal_mods(mods,&target,&handle,life).await;
                        life
                    },
                    None=>0,
                };
                self.entries.lock().await.insert(target.key.clone(),Entry {generation:target.generation,handle:EntryHandle::Terminal {target:target.clone(),handle:handle.clone()},lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:life,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
                    mode_source:crate::terminal_facts::ModeSource {binding:crate::terminal_facts::LocalTerminalFacts::binding(&target),transcript:target.transcript.clone(),confirmed:None},live:self.live_sender(&target.name),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});handle
            }
        };
        let snapshot=handle.snapshot().await?;
        self.bind_plugin(&target.name).await?;
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    /// Fecha a sessão no Rust: o ator para e solta a trava; o cano segue vivo.
    pub async fn close(&self,key:&str,generation:u64) -> Result<Value,RuntimeError> {
        let barrier = self.barrier(key).await;
        barrier.cancel();
        let _guard = barrier.lock().await;
        self.close_locked(key,generation,false).await
    }
    /// Encerrar a sessão: o ator para e o processo dela morre, com os arquivos do cano. Sem a sessão
    /// aberta aqui não há o que matar (`killed: false`): quem pediu decide.
    pub async fn close_with_kill(&self,key:&str,generation:u64) -> Result<Value,RuntimeError> {
        self.close_with_kill_as(key,generation,false).await
    }
    /// `refuse_transfer`: a porta retida pela troca de conversa recusa na hora (o `DELETE` do dono), em vez
    /// de esperar a troca terminar e encerrar a sessão que acabou de mudar.
    async fn close_with_kill_as(&self,key:&str,generation:u64,refuse_transfer:bool) -> Result<Value,RuntimeError> {
        let codex = match self.entries.lock().await.get(key) {
            Some(Entry { generation:open,handle:EntryHandle::CodexTerminal { target,.. },.. }) if *open == generation=>Some((**target).clone()),
            _=>None,
        };
        if let Some(target) = codex {
            // Codex com terminal: pane, app-server, sidecar e fila desta vida, pela vigia. Sem ela, nada encerra.
            let watcher = self.watcher.get().cloned().ok_or_else(||failure("close_kill"))?;
            return watcher.kill(self,&target).await;
        }
        if !refuse_transfer {self.barrier(key).await.cancel();}
        let name=self.entries.lock().await.get(key).filter(|e|e.generation==generation).map(|e|e.name.clone());
        let _ingress=match &name {
            Some(name) if refuse_transfer=>Some(super::lifecycle::ClosedIngress::acquire_unless_transfer(&self.ingress,name).await?),
            Some(name)=>Some(super::lifecycle::ClosedIngress::acquire(&self.ingress,name).await?),
            None=>None,
        };
        if refuse_transfer {self.barrier(key).await.cancel();}
        let barrier = self.barrier(key).await;
        let _guard = barrier.lock().await;
        let result=self.close_locked(key,generation,true).await;
        if let (Err(error),Some(name))=(&result,&name) {super::actor::mark_live_error(&self.live_sender(name),&error.code,&error.message);}
        result
    }
    async fn close_locked(&self,key:&str,generation:u64,kill:bool) -> Result<Value,RuntimeError> {
        self.close_locked_as(key,generation,kill,false).await
    }
    /// `park`: mata como o encerrar, mas a fila e o arquivo da sessão ficam para a próxima vida.
    async fn close_locked_as(&self,key:&str,generation:u64,kill:bool,park:bool) -> Result<Value,RuntimeError> {
        let (handle,lease_path,name,life,runtime_target,cano_birth,recipe,_settings_owner) = match self.entries.lock().await.get(key) {
            None=>{
                // Fechar a estacionada é fechar: o próximo envio não a reabre.
                if let Ok(mut parked)=self.barrier(key).await.parked.lock() {*parked=None;}
                return Ok(if kill { json!({"closed":true,"killed":false}) } else { json!({"closed":true}) });
            },
            Some(entry) if entry.generation == generation=>(entry.handle.clone(),entry.lease_path.clone(),entry.name.clone(),entry.mods_life,entry.runtime_target.clone(),entry.cano_birth,entry.relaunch.clone(),entry.settings_owner.clone()),
            _=>return Err(failure("runtime_generation")),
        };
        let mut closed_life=match &handle {
            EntryHandle::Terminal {target,..}=>Some(super::lifecycle::ClosedLife {name:name.clone(),binding:crate::terminal_facts::LocalTerminalFacts::binding(target),pid:i64::from(target.metadata.agent_pid),birth:target.metadata.agent_birth,state_path:target.state_path.clone(),lease_path:target.lease_path.clone(),projection:target.projection_dir.clone()}),
            EntryHandle::Headless(_) if runtime_target.as_ref().is_some_and(|target|target.provider=="claude")=> {
                let target=runtime_target.as_ref().unwrap();
                let binding=self.entries.lock().await.get(key).and_then(|entry|entry.live.borrow().as_ref().and_then(|live|live.binding.clone()));
                match binding {
                    Some(binding)=>Some(super::lifecycle::ClosedLife {name:name.clone(),binding,pid:i64::from(target.binding.pid),birth:cano_birth.ok_or_else(||failure("lifecycle_process_birth_missing"))?,state_path:target.state_path.clone(),lease_path:target.lease_path.clone(),projection:target.projection_dir.clone()}),
                    // Só soltar o ator (o que morreu antes de publicar a conversa) não precisa do vínculo.
                    None if !kill=>None,
                    None=>return Err(failure("lifecycle_binding_unavailable")),
                }
            },
            _=>None,
        };
        if let Some(plugin) = &self.plugin { plugin.forget(key).map_err(|error|failure(error.0))?; }
        // Processo que o Rust não comprova ser desta vida: o ator para e a sessão sai daqui, mas nada é
        // morto (`killed: false`); quem pediu decide.
        let mut foreign = false;
        let stopped = match (&handle,kill) {
            (EntryHandle::Headless(handle),true) if runtime_target.as_ref().is_some_and(|target|target.provider=="claude")=> {
                let target=runtime_target.as_ref().unwrap();
                let born=cano_birth.ok_or_else(||failure("lifecycle_process_birth_missing"))?;
                let procs=self.processes.clone();
                let ours=cano_is_ours(procs.as_ref(),target,born,key);
                if !matches!(ours,Ok(true)) {
                    foreign = true;
                    if let Err(error) = handle.stop().await { tracing::warn!(key,code=%error.code,"ator parou com erro ao soltar processo alheio"); }
                    Err(ours.err().unwrap_or_else(||failure("lifecycle_process_changed")))
                } else {
                let stopped=handle.stop().await;
                if let Err(error)=&stopped && wait_lease(&target.lease_path).await.is_err() {return Err(error.clone());}
                let mut cano=process::Cano {pid:target.binding.pid,escuta:target.binding.escuta.clone(),token:target.binding.token.clone(),ts:0.0,versao:target.binding.versao,extra:Default::default()};
                cano.extra.insert("process_birth".into(),json!(born));
                // O ator já parou: erro daqui em diante é o valor de `stopped`, para a entrada sair abaixo em vez
                // de ficar com o ator morto.
                let killed:Result<(),RuntimeError>=async {
                    let dir=super::lifecycle::claude_sidecar_dir()?;
                    process::kill(&cano,key,&dir).await.map_err(|e|RuntimeError::new(e.code(),"o cano da sessão não foi encerrado"))?;
                    if let Some(recipe)=&recipe {let (pid,birth)=recipe.agent_identity();super::lifecycle::wait_absent(procs.as_ref(),pid,birth,"lifecycle_agent_still_alive").await?;}
                    Ok(())
                }.await;
                // O cano morreu e o Claude não: a vida fechada é a dele, e nada sobe por cima enquanto ele viver.
                if let (Err(error),Some(closed),Some(recipe))=(&killed,closed_life.as_mut(),&recipe) && error.code=="lifecycle_agent_still_alive" {
                    (closed.pid,closed.birth)=recipe.agent_identity();
                }
                killed
                }
            },
            (EntryHandle::Headless(handle),true)=>handle.stop_killing().await,
            (EntryHandle::Terminal {target,handle},true)=> {
                let procs=crate::list::procs::SystemProcs::default();
                let before=super::lifecycle::pane(&crate::terminal_input::ProcessIo::default(),&target.binding).await?;
                use crate::list::procs::ProcessView;
                if before.dead || procs.recipe(i64::from(target.metadata.agent_pid)).map_err(|_|failure("lifecycle_process_read"))?.birth!=target.metadata.agent_birth {return Err(failure("lifecycle_process_changed"));}
                handle.stop().await?;
                // Como o Python, antes de matar: a pergunta aberta desta vida não reabre o stepper numa
                // sessão futura com a mesma conversa. Sem conseguir apagar, só o diário: o fechamento segue.
                if let Some(path)=crate::state::ask::sidecar_path(&target.transcript)
                    && let Err(error)=std::fs::remove_file(&path) && error.kind()!=std::io::ErrorKind::NotFound {
                    tracing::warn!(key,session=%name,kind=?error.kind(),"pergunta aberta não foi apagada ao fechar");
                }
                let killed=super::lifecycle::kill_terminal(&crate::terminal_input::ProcessIo::default(),&procs,&target.binding,
                    i64::from(target.metadata.agent_pid),target.metadata.agent_birth).await;
                if let Err(error)=&killed {super::actor::mark_live_error(&self.live_sender(&name),&error.code,&error.message);}
                killed
            },
            // Encerrar o Codex com terminal é da vigia (`close_with_kill`).
            (EntryHandle::CodexTerminal {..},true)=>return Err(failure("close_kill")),
            (EntryHandle::CodexTerminal { cell,handle,.. },false)=>{
                // A reconexão em voo para antes de a posse sair daqui.
                cell.revoke();
                handle.stop().await
            },
            _=>handle.stop().await,
        };
        if let Err(error) = stopped {
            // `stop` sempre junta a tarefa do ator: se ele saiu por erro, a posse acaba com ele. Sem
            // isto a entrada morta ficava para sempre, a sessão não reabria e o retrato de eventos
            // de todas as sessões caía. Só solta depois de a trava estar livre de fato.
            if wait_lease(&lease_path).await.is_err() {
                tracing::warn!(key,code=%error.code,"ator terminou mas a trava não liberou em 3 s; sessão segue presa");
                return Err(error);
            }
            if kill && closed_life.is_some() && !foreign {
                if let Some(store)=&self.interactions {store.forget(key).map_err(crate::interactions::runtime_error)?;}
                self.entries.lock().await.remove(key);
                if let Some(mods)=&self.mods {mods.forget(&name,life);}
                if let Some(closed)=closed_life.clone() {*self.barrier(key).await.closed.lock().map_err(|_|failure("lifecycle_closed_record"))?=Some(closed);}
                super::actor::mark_live_error(&self.live_sender(&name),&error.code,&error.message);
                return Err(error);
            }
            if kill {
                // O ator parou e soltou a trava: a sessão saiu daqui, só o processo não morreu. Responder
                // erro deixava o Python achando que ela seguia aqui; `killed: false` e quem pediu decide.
                tracing::warn!(key,code=%error.code,"sessão fechada sem encerrar o processo");
                if let Some(store) = &self.interactions { store.forget(key).map_err(crate::interactions::runtime_error)?; }
                self.entries.lock().await.remove(key);
                self.clear_live(&name).await;
                if let Some(mods) = &self.mods { mods.forget(&name,life); }
                return Ok(json!({"closed":true,"killed":false}));
            }
            tracing::warn!(key,code=%error.code,"ator do runtime já tinha terminado; sessão liberada");
        }
        if let Some(closed)=&closed_life {
            if let Some(store)=&self.interactions {store.forget_binding(&closed.binding).map_err(crate::interactions::runtime_error)?;}
            self.forget_claude_rate(&closed.binding)?;
            *self.barrier(key).await.closed.lock().map_err(|_|failure("lifecycle_closed_record"))?=Some(closed.clone());
            if kill && !park {
                let lease=wait_lease(&closed.lease_path).await?;
                let mut store=Store::open(&closed.state_path,&closed.projection,QueueState::new(key,generation,&closed.name,vec![])).map_err(|_|failure("queue_io"))?;
                if store.state().owner_key!=key || store.state().generation!=generation {return Err(failure("runtime_generation"));}
                let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
                store.exec(generation,&format!("close-clear:{key}:{generation}"),clock,Action::Clear).map_err(|_|failure("queue_io"))?;
                store.exec(generation,&format!("close-record:{key}:{generation}"),clock,Action::AdvanceLifecycle {generation,state:json!({"terminal_lifecycle":{"closed":true,"conversation":closed.binding.conversation},"view":{"alive":false,"conversation":closed.binding.conversation}})}).map_err(|_|failure("queue_io"))?;
                if let Some(target)=&runtime_target {super::lifecycle::delete_sidecar(target,&closed.binding.conversation)?;}
                drop(store);drop(lease);
            }
        } else if let Some(store) = &self.interactions { store.forget(key).map_err(crate::interactions::runtime_error)?; }
        self.entries.lock().await.remove(key);
        self.terminal_problems.lock().unwrap_or_else(|e|e.into_inner()).remove(key);
        // Codex com terminal fora do Rust: o hub devolve o estado à observação do Python.
        if matches!(handle,EntryHandle::CodexTerminal {..}) { self.mark_codex_owner(&name,key,generation,false); }
        self.clear_live(&name).await;
        // A sessão saiu do Rust: os apps perdem a faixa e os pedidos voltam a não ter dono (S9). Com ou sem
        // terminal, esquece só esta vida: outra sessão que tenha tomado o nome (outra vida) fica.
        if let Some(mods) = &self.mods { mods.forget(&name,life); }
        Ok(if kill { json!({"closed":true,"killed":true}) } else { json!({"closed":true}) })
    }
    /// Ciclo dos subagentes da sessão Claude sem terminal aberta aqui, por agentId:
    /// `{"<agentId>":{"state":"running|completed|failed|stopped","run_id":"…","end_time":<ms>|null}}`.
    /// `None` = não há sessão Claude sem terminal com esse nome neste servidor.
    pub async fn claude_subagents(&self,name:&str) -> Option<Result<Value,RuntimeError>> {
        let handle={
            let entries=self.entries.lock().await;
            let mut matches=entries.values().filter(|entry|entry.name==name && entry.provider=="claude");
            let entry=matches.next()?;if matches.next().is_some() {return None;}
            match &entry.handle {EntryHandle::Headless(handle)=>handle.clone(),_=>return None}
        };
        Some(handle.view().await.map(|view|view["subagents"].clone()))
    }

    /// Leitura do catálogo também com o processo estacionado; não passa pela porta de escrita.
    pub async fn claude_catalog_view(&self,name:&str) -> Option<Result<(Value,Value),RuntimeError>> {
        let found = {
            let entries = self.entries.lock().await;
            let entry = entries.values().filter(|e| e.name == name && e.provider == "claude").max_by_key(|e| e.generation)?;
            match &entry.handle {
                EntryHandle::Headless(handle) => Some((entry.runtime_target.as_ref()?.metadata.clone(), handle.clone())),
                _ => None,
            }
        };
        let (metadata,handle) = found?;
        Some(handle.view().await.map(|view| (metadata,view)))
    }

    pub(crate) async fn catalog_provider(&self,name:&str) -> Option<String> {
        self.entries.lock().await.values().filter(|entry| entry.name == name).max_by_key(|entry| entry.generation).map(|entry| entry.provider.clone())
    }
    /// `session_key` é a conversa, como na descoberta: o hub a compara com o `conversation` do vínculo.
    pub async fn claude_info(&self,name:&str)->Option<crate::transcript::InternalInfo> {
        let entries=self.entries.lock().await;
        let mut matches=entries.values().filter(|entry|entry.name==name && entry.provider=="claude");
        let entry=matches.next()?;if matches.next().is_some() {return None;}
        match &entry.handle {
            EntryHandle::Terminal {target,..}=>Some(crate::transcript::InternalInfo {provider:"claude".into(),jsonl:Some(target.transcript.clone()),session_key:target.binding.conversation.clone(),
                history:json!({"queue":target.projection_dir.join(format!("{}.jsonl",name))}),headless:false,lifecycle_id:None,cwd:None}),
            EntryHandle::Headless(_)=> {
                let live=entry.live.borrow().clone()?;
                let binding=live.binding.as_ref().filter(|binding|binding.key==entry.mode_source.binding.key && binding.generation==entry.generation && !binding.conversation.is_empty())?;
                let jsonl=entry.mode_source.transcript.with_file_name(format!("{}.jsonl",binding.conversation));
                Some(crate::transcript::InternalInfo {provider:crate::transcript::Provider::ClaudeHeadless.as_str().into(),jsonl:Some(jsonl),session_key:binding.conversation.clone(),history:json!({"queue":entry.lease_path.parent()?.parent()?.join(format!("{name}.jsonl"))}),headless:true,lifecycle_id:None,cwd:None})
            },
            _=>None,
        }
    }
    fn forget_claude_rate(&self,binding:&crate::interactions::InteractionBinding)->Result<(),RuntimeError> {
        let weak=self.hubs.get().ok_or_else(||failure("lifecycle_hubs_unbound"))?;
        // Hubs já derrubados (encerramento do servidor) levaram as taxas junto: nada a esquecer.
        if let Some(hubs)=weak.upgrade() {hubs.plugin_rates().forget_binding(binding);}
        Ok(())
    }
    fn rebind_claude_hub(&self,target:&super::terminal::TerminalTarget)->Result<(),RuntimeError> {
        let hubs=self.hubs.get().and_then(crate::side::WeakHubs::upgrade).ok_or_else(||failure("lifecycle_hubs_unbound"))?;
        hubs.rebind_claude(&target.name,&target.binding.conversation,target.transcript.clone(),target.projection_dir.join(format!("{}.jsonl",target.name)))
    }
    /// Chamado pelo ator serial: não para nem consulta o próprio ator e não espera reabridor HTTP.
    pub(super) async fn commit_claude_sid(&self,target:&RuntimeTarget,queue:&QueueActor,patch:&Value)->Result<RuntimeTarget,RuntimeError> {
        let barrier=self.barrier(&target.key).await;
        let _guard=barrier.try_lock().map_err(|_|failure("lifecycle_sid_in_transfer"))?;
        let mut entries=self.entries.lock().await;
        let entry=entries.get_mut(&target.key).filter(|entry|entry.generation==target.generation && entry.name==target.name && entry.provider=="claude").ok_or_else(||failure("runtime_binding"))?;
        let EntryHandle::Headless(handle)=&entry.handle else {return Err(failure("lifecycle_provider"));};
        if !entry.runtime_target.as_ref().is_some_and(|current|current.binding==target.binding && current.metadata["session_id"]==target.metadata["session_id"]) {return Err(failure("runtime_binding"));}
        let next_sid=patch["session_id"].as_str().filter(|sid|!sid.is_empty()).ok_or_else(||failure("lifecycle_conversation"))?;
        let previous=target.metadata["session_id"].as_str().ok_or_else(||failure("lifecycle_conversation"))?;
        let state=queue.snapshot().await.map_err(|_|failure("queue_io"))?;
        if state.owner_key!=target.key || state.generation!=target.generation {return Err(failure("runtime_generation"));}
        let mut captured=target.clone();
        if let Some(meta)=state.runtime_state.get("metadata").filter(|meta|meta["key"]==target.key && meta["cano"]["pid"]==target.binding.pid) {captured.metadata=meta.clone();}
        let lease=queue.lease();let values=patch.clone();let original=captured.clone();
        let saved=tokio::task::spawn_blocking(move || {let _lease=lease;super::lifecycle::patch_sidecar(&original,&values)}).await.map_err(|_|failure("lifecycle_worker"))??;
        if saved["updated"]!=true {return Err(failure("lifecycle_sidecar_stale"));}
        let mut next=target.clone();next.generation=target.generation.checked_add(1).ok_or_else(||failure("runtime_generation"))?;
        next.metadata=saved["metadata"].clone();next.metadata["lifecycle_pending"]=json!(false);
        // Plugins e skills escolhidos seguem a conversa nova; sem isso, reabri-la viria com tudo ligado.
        let (sid,settings)=(next_sid.to_owned(),next.metadata["claude_settings"].clone());
        if let Err(code)=tokio::task::spawn_blocking(move||crate::claude_customizations::remember(&sid,&settings).map_err(|(code,_)|code)).await.map_err(|_|"claude_customizations_failed").and_then(|r|r) {
            tracing::warn!(key=%target.key,session=%target.name,code,"escolhas da conversa não passaram à conversa nova do /clear");
        }
        next.transcript=target.transcript.with_file_name(format!("{next_sid}.jsonl"));
        let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
        queue.exec(target.generation,&format!("sid-commit:{}:{}:{next_sid}",target.key,target.generation),clock,Action::AdvanceLifecycle {generation:next.generation,state:json!({"conversation_reset":{"previous":previous,"next":next_sid},"metadata":next.metadata})}).await.map_err(|_|failure("queue_io"))?;
        entry.handle=EntryHandle::Headless(handle.clone().with_generation(next.generation));
        entry.generation=next.generation;entry.runtime_target=Some(next.clone());
        entry.mode_source.binding=crate::interactions::InteractionBinding {key:next.key.clone(),conversation:next_sid.into(),generation:next.generation};entry.mode_source.transcript=next.transcript.clone();
        // A loja do plugin segue a conversa nova, como na abertura (`bind_plugin`): presa à antiga, todo fato
        // lido dava `erro_interacao_desatualizada` até a sessão reabrir.
        // A troca já está gravada: falha aqui vai ao log e ao diário, e o resto da troca segue.
        if let Some(plugin)=&self.plugin && let Err(error)=plugin.bind(&entry.mode_source.binding) {
            tracing::error!(key=%target.key,session=%target.name,code=error.0,"troca de conversa: a loja do plugin não seguiu a conversa nova");
            self.diag.report("rust.plugin_rebind_failed",&target.name,error.0,"a loja do plugin não seguiu a conversa nova do /clear");
        }
        self.forget_claude_rate(&crate::interactions::InteractionBinding {key:target.key.clone(),conversation:previous.into(),generation:target.generation})?;
        Ok(next)
    }
    /// Native SID novo, incluindo `/clear` digitado diretamente na TUI; sem tecla nem política Python.
    pub async fn reconcile_claude(&self,name:&str,key:&str,conversation:&str,generation:u64)->Result<Value,RuntimeError> {
        let target=match self.entries.lock().await.get(key).filter(|entry|entry.name==name && entry.generation==generation).map(|entry|entry.handle.clone()) {
            Some(EntryHandle::Terminal {target,..}) if target.binding.conversation==conversation=>target,
            _=>return Ok(json!({"in_transfer":true})),
        };
        // A troca vem do próprio Claude Code: o plugin passa a informar a conversa nova no `/pull` (o `/clear`
        // encerra a antiga com `session.end`). Sem ele, vale o registro nativo do processo.
        let binding=crate::terminal_facts::LocalTerminalFacts::binding(&target);
        let (reported,proved)=match &self.plugin {
            Some(plugin)=>match (plugin.reported_conversation(&binding),plugin.conversation_proved(&binding)) {
                (Ok(reported),proved)=>(reported,proved.unwrap_or(false)),
                (Err(error),_)=>{
                    if crate::warn_limit::allow(Some(key),error.0) {tracing::warn!(key,session=name,code=error.0,"troca de conversa: sinal do plugin ilegível; vale o registro nativo");}
                    (None,false)
                },
            },
            None=>(None,false),
        };
        // O plugin prova a conversa atual (`/pull` aberto com ela, sem `session.end` depois): nada a reler.
        if reported.is_none() && proved {return Ok(json!({"changed":false}));}
        let sid=match &reported {
            Some(sid)=>sid.clone(),
            None=>match super::lifecycle::read_native_conversation(&target.metadata.config_dir,i64::from(target.metadata.agent_pid)) {
                Ok(sid)=>sid,
                // Registro que este Claude Code não grava, ou lido no meio da escrita: o próximo tique relê. Sem o
                // plugin, é o único sinal de `/clear`: a falha vai ao log, uma vez por sessão e código.
                Err(error) if error.code=="lifecycle_native_read"=> {
                    if crate::warn_limit::allow(Some(key),&error.code) {tracing::warn!(key,session=name,code=%error.code,"troca de conversa: registro nativo ilegível e sem prova do plugin");}
                    return Ok(json!({"changed":false}));
                },
                Err(error)=>return Err(error),
            },
        };
        if sid==conversation {return Ok(json!({"changed":false}));}
        let _ingress=super::lifecycle::ClosedIngress::acquire(&self.ingress,name).await?;
        let barrier=self.barrier(key).await;let _guard=barrier.lock().await;
        let current=self.interaction_binding_readonly(name).await?;
        if !super::lifecycle::matches_life(&current,key,conversation,generation) {return Ok(json!({"in_transfer":true}));}
        let procs=crate::list::procs::SystemProcs::default();
        if reported.is_some() {
            // O plugin é o do processo da sessão: basta ele seguir vivo, com o mesmo nascimento.
            use crate::list::procs::ProcessView;
            if procs.recipe(i64::from(target.metadata.agent_pid)).map_err(|_|failure("lifecycle_process_read"))?.birth!=target.metadata.agent_birth {return Err(failure("lifecycle_process_changed"));}
        } else {
            let confirmed=super::lifecycle::native_conversation(&target.metadata.config_dir,i64::from(target.metadata.agent_pid),target.metadata.agent_birth,&procs)?;
            if confirmed!=sid {return Err(failure("lifecycle_native_changed"));}
        }
        let pane=super::lifecycle::pane(&crate::terminal_input::ProcessIo::default(),&target.binding).await?;
        if pane.dead {return Err(failure("lifecycle_native_dead"));}
        let retained=self.entries.lock().await.get(key).and_then(|entry|entry.relaunch.clone());
        self.close_locked(key,generation,false).await?;
        let lease=wait_lease(&target.lease_path).await?;
        let mut store=Store::open(&target.state_path,&target.projection_dir,QueueState::new(key,generation,name,vec![])).map_err(|_|failure("queue_io"))?;
        if store.state().owner_key!=key || store.state().generation!=generation {return Err(failure("runtime_generation"));}
        let mut next=target.clone();next.generation=generation.checked_add(1).ok_or_else(||failure("runtime_generation"))?;
        next.binding.generation=next.generation;next.binding.conversation=sid.clone();
        next.transcript=target.transcript.parent().ok_or_else(||failure("lifecycle_transcript"))?.join(format!("{sid}.jsonl"));
        let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
        let phase=format!("claude-sid:{key}:{generation}:{sid}");
        store.exec(generation,&format!("{phase}:prepare"),clock,Action::Prepare {id:phase.clone(),payload:json!({"kind":"claude_sid","conversation":sid,"previous":conversation,"generation":generation}),entry_id:None}).map_err(|_|failure("queue_io"))?;
        store.exec(generation,&format!("{phase}:clear"),clock,Action::Clear).map_err(|_|failure("queue_io"))?;
        store.exec(generation,&format!("{phase}:advance"),clock,Action::AdvanceLifecycle {generation:next.generation,state:json!({"terminal_lifecycle":{"target":super::lifecycle::target_value(&next),"conversation":sid}})}).map_err(|_|failure("queue_io"))?;
        drop(store);drop(lease);
        self.open_terminal_locked(next.clone()).await?;
        if let Some(entry)=self.entries.lock().await.get_mut(key) {entry.relaunch=retained;}
        self.rebind_claude_hub(&next)?;
        let _=self.events.send(RuntimeEvent {key:key.into(),generation:next.generation,revision:0,channel:"reset".into(),data:json!({"conversation":sid,"generation":next.generation})});
        Ok(json!({"changed":true,"conversation":sid,"generation":next.generation}))
    }

    /// Admitida: a tarefa continua mesmo que o cliente HTTP deixe de esperar.
    pub async fn reopen_claude(self:Arc<Self>,name:String,key:String,generation:u64,operation_id:String)->Result<Value,RuntimeError> {
        tokio::spawn(async move {self.reopen_claude_inner(&name,&key,generation,&operation_id,Reopen::Bypass).await}).await.map_err(|_|failure("lifecycle_panic"))?
    }
    /// Claude com terminal sem prova da conversa (registro nativo do agente sumiu e o plugin não
    /// responde): reabre na mesma conversa e no mesmo modo, parado e com mensagem esperando, e a vida
    /// nova regrava o registro. No máximo uma por `HEAL_BACKOFF` por sessão; cada tentativa vai ao diário.
    pub async fn heal_claude(self:Arc<Self>,name:String,key:String,generation:u64)->Result<Value,RuntimeError> {
        {
            let mut attempts=self.heal_attempts.lock().unwrap_or_else(|e|e.into_inner());
            let now=std::time::Instant::now();
            if attempts.get(&key).is_some_and(|at|now.duration_since(*at)<HEAL_BACKOFF) {return Err(failure("heal_backoff"));}
            attempts.insert(key.clone(),now);
        }
        let diag=self.diag.clone();
        tracing::warn!(key,session=%name,code="terminal_heal_started","sessão sem entrega segura: o Claude é reaberto na mesma conversa e modo");
        diag.report("rust.terminal_heal",&name,"started","sessão sem entrega segura: reabertura na mesma conversa e modo");
        let id=format!("heal:{key}:{generation}:{}",crate::mods::state::random_hex(8));
        let (session,owner)=(name.clone(),key.clone());
        let result=tokio::spawn(async move {self.reopen_claude_inner(&session,&owner,generation,&id,Reopen::Heal).await}).await.map_err(|_|failure("lifecycle_panic")).and_then(|r|r);
        match &result {
            Ok(_)=>{
                tracing::info!(key,session=%name,code="terminal_heal_done","Claude reaberto na mesma conversa; a fila segue na vida nova");
                diag.report("rust.terminal_heal",&name,"relaunched","Claude reaberto na mesma conversa e modo");
            },
            Err(error)=>{
                tracing::warn!(key,session=%name,code=%error.code,reason=%error.message,"reabertura por entrega sem prova falhou");
                diag.report("rust.terminal_heal",&name,&error.code,"reabertura por entrega sem prova falhou");
            },
        }
        result
    }
    async fn reopen_claude_inner(&self,name:&str,key:&str,generation:u64,id:&str,reopen:Reopen)->Result<Value,RuntimeError> {
        use crate::list::procs::ProcessView;
        let _ingress=super::lifecycle::ClosedIngress::acquire(&self.ingress,name).await?;
        let barrier=self.barrier(key).await;let _guard=barrier.lock().await;
        let entry={
            let entries=self.entries.lock().await;
            entries.get(key).filter(|entry|entry.name==name).map(|entry|entry.handle.clone()).ok_or_else(||failure("runtime_binding"))?
        };
        let heal=reopen==Reopen::Heal;
        let (target,handle)=match entry {
            EntryHandle::Terminal {target,handle}=>(target,handle),
            EntryHandle::Headless(_) if !heal=>return self.reopen_headless_locked(name,key,generation,id,Some("bypassPermissions"),false,None,0).await,
            _=>return Err(failure("lifecycle_provider")),
        };
        if target.generation!=generation {
            if heal || generation.checked_add(1)!=Some(target.generation) {return Err(failure("runtime_generation"));}
            let previous=handle.control(id.into(),"set_permission_mode".into(),json!({"mode":"bypassPermissions"})).await?;
            if previous.disposition==Disposition::Accepted && previous.payload["reopened"]==true {return Ok(previous.payload);}
            return Err(failure("runtime_generation"));
        }
        let next_generation=generation.checked_add(1).ok_or_else(||failure("runtime_generation"))?;
        let (phase,kind,guard)=if heal {(format!("claude-heal:{id}"),"claude_heal","heal_guard")} else {(format!("claude-reopen:{id}"),"claude_reopen","lifecycle_guard")};
        let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
        handle.lifecycle(format!("{phase}:prepare"),Action::Prepare {id:phase.clone(),payload:json!({"logical_id":id,"kind":kind,"conversation":target.binding.conversation,"generation":generation}),entry_id:None}).await?;
        handle.control(format!("guard:{id}"),guard.into(),json!({})).await?;
        let io=crate::terminal_input::ProcessIo::default();
        let procs=crate::list::procs::SystemProcs::default();
        let pane=super::lifecycle::pane(&io,&target.binding).await?;
        if pane.dead || pane.copy {return Err(failure("lifecycle_busy"));}
        // A cura conserva o modo que o rodapé mostra; rodapé ilegível não reabre num modo inventado.
        let mode=if heal {super::lifecycle::permission_mode(&io,&target.binding).await?} else {"bypassPermissions".to_owned()};
        let actual=procs.recipe(i64::from(target.metadata.agent_pid)).map_err(|_|failure("lifecycle_process_read"))?;
        let argv_sid=super::claude_relaunch::Recipe::argument_conversation(&actual)?;
        let mut recipe=super::claude_relaunch::Recipe::capture(&procs,pane.pid,i64::from(target.metadata.agent_pid),target.metadata.agent_birth,&argv_sid,false)?;
        let directory=target.state_path.parent().ok_or_else(||failure("relaunch_settings_copy"))?;
        if mode=="bypassPermissions" {recipe.policy_preflight(&target.metadata.config_dir)?;}
        recipe.preserve_settings(directory)?;
        let mut launch=recipe.launch(&target.binding.conversation,&mode)?;
        let choices=super::lifecycle::terminal_choices(&target)?;
        if let Some(choices)=&choices {recipe.apply_choices(&mut launch,choices.model.as_deref(),choices.effort.as_deref())?;}
        else {tracing::warn!(key,session=name,code="relaunch_choices_unpublished","modelo/esforço sem sidecar comprovado; conservada a receita viva sem padrão inventado");}
        let handoff=super::claude_relaunch::Handoff::prepare().map_err(|_|failure("relaunch_ipc"))?;
        let endpoint=handoff.endpoint().to_owned();
        let (staged,pid)=super::lifecycle::stage(&io,&target.binding,&endpoint).await?;
        let deadline=tokio::time::Instant::now()+Duration::from_secs(5);
        let helper=loop {
            match procs.recipe(pid) {
                Ok(recipe) if recipe.executable==std::env::current_exe().map_err(|_|failure("relaunch_binary"))? && recipe.argv.get(1).is_some_and(|a|a==super::claude_relaunch::RELAUNCH_FLAG)=>break recipe,
                _ if tokio::time::Instant::now()<deadline=>tokio::time::sleep(Duration::from_millis(20)).await,
                _=>return Err(failure("relaunch_helper_identity")),
            }
        };
        let recipe=Arc::new(recipe);
        if launch.cgroup.is_some() && helper.cgroup!=launch.cgroup {return Err(failure("relaunch_scope_changed"));}
        let consumed=handoff.consume(&helper,&procs,&super::claude_relaunch::LaunchRecipe::from(&launch)).await.map_err(|_|failure("relaunch_ipc_consumption"))?;
        // Ainda sem parar a origem: reconfere ociosidade e toda a receita após criar o destino.
        handle.control(format!("guard-final:{id}"),guard.into(),json!({})).await?;
        recipe.verify(&procs)?;
        if super::lifecycle::terminal_choices(&target)?!=choices {return Err(failure("lifecycle_choices_changed"));}
        handle.lifecycle(format!("{phase}:dispatch"),Action::BeginDispatch {id:phase.clone(),wire_id:phase.clone(),staged:false}).await?;
        let original=procs.recipe(i64::from(target.metadata.agent_pid)).map_err(|_|failure("lifecycle_process_read"))?;
        self.close_locked(key,generation,false).await?;
        let lease=wait_lease(&target.lease_path).await?;
        let mut store=Store::open(&target.state_path,&target.projection_dir,QueueState::new(key,generation,name,vec![])).map_err(|_|failure("queue_io"))?;
        if store.state().owner_key!=key || store.state().generation!=generation {return Err(failure("runtime_generation"));}
        // O Claude novo já subiu no destino: a origem morreu antes, então a limpeza não pode matá-lo.
        let started=std::sync::atomic::AtomicBool::new(false);
        let result=async {
            super::lifecycle::kill_pane(&io,&target.binding,&original,&procs).await?;
            consumed.start().await.map_err(|_|failure("relaunch_start_unknown"))?;
            let mut next=target.clone();next.generation=next_generation;
            next.binding.generation=next.generation;next.binding.pane=staged.clone();
            // `--resume` de conversa longa demora a mostrar o prompt.
            let deadline=tokio::time::Instant::now()+Duration::from_secs(60);
            let local=crate::terminal_facts::LocalTerminalFacts::new(Arc::new(io.clone()),self.plugin.clone(),Some(self.terminal_mode_reader()));
            let (agent,birth)=loop {
                if let Ok((agent,birth))=super::lifecycle::launched_agent(&procs,pid,&target.binding.conversation) {
                    started.store(true,std::sync::atomic::Ordering::Relaxed);
                    next.metadata.agent_pid=u32::try_from(agent).map_err(|_|failure("lifecycle_process_read"))?;
                    next.metadata.agent_birth=birth;
                    if local.check(&next,&next.binding).await.is_ok()
                        && super::lifecycle::permission_mode(&io,&next.binding).await.is_ok_and(|shown|shown==mode) {break (agent,birth);}
                }
                if tokio::time::Instant::now()>=deadline {return Err(failure("relaunch_confirmation"));}
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            next.metadata.agent_pid=u32::try_from(agent).map_err(|_|failure("lifecycle_process_read"))?;next.metadata.agent_birth=birth;
            recipe.pin_settings(agent,birth)?;
            next.metadata.legacy_import_after=Some(birth);
                store.exec(generation,&format!("{phase}:advance"),clock,Action::AdvanceLifecycle {generation:next.generation,state:json!({"terminal_lifecycle":{"target":super::lifecycle::target_value(&next),"conversation":next.binding.conversation,"permission_mode":mode}})}).map_err(|_|failure("queue_io"))?;
            let value=if heal {json!({"mode":mode,"reopened":true,"healed":true})}
                else {json!({"mode":"bypassPermissions","current":"bypassPermissions","previous_non_plan":"bypassPermissions","reopened":true})};
            let reply=RuntimeReply {operation_id:id.into(),disposition:Disposition::Accepted,payload:value.clone()};
            drop(store);drop(lease);
            self.open_terminal_locked(next.clone()).await?;
            self.rebind_claude_hub(&next)?;
            if let Some(entry)=self.entries.lock().await.get_mut(key) {entry.relaunch=Some(recipe.clone());}
            let EntryHandle::Terminal {handle,..}=self.entry(key,next.generation).await? else {return Err(failure("lifecycle_provider"));};
            // A cura não tem pedido do app para fechar: só a fase dela.
            if !heal {handle.lifecycle(format!("{phase}:finish-root"),Action::Finish {id:id.into(),status:super::queue::Status::Accepted,result:serde_json::to_value(reply).unwrap()}).await?;}
            handle.lifecycle(format!("{phase}:finish"),Action::Finish {id:phase.clone(),status:super::queue::Status::Accepted,result:value.clone()}).await?;
            if let Some(memory)=&self.permissions {memory.lock().map_err(|_|failure("permission_store_poisoned"))?.observe(&target.binding.conversation,&mode);}
            let _=self.events.send(RuntimeEvent {key:key.into(),generation:next.generation,revision:0,channel:"reset".into(),data:json!({"conversation":next.binding.conversation,"generation":next.generation})});
            Ok(value)
        }.await;
        if let Err(error)=&result {
            // A vida antiga já não está aberta aqui; a falha nunca confirma modo original nem bypass.
            // Destino com o Claude já de pé fica vivo: é a conversa do usuário e a origem já morreu.
            let alive=started.load(std::sync::atomic::Ordering::Relaxed);
            let message=if alive {format!("{} O Claude novo subiu no terminal e ficou aberto, sem dono aqui: confira a sessão.",error.message)} else {error.message.clone()};
            super::actor::mark_live_error(&self.live_sender(name),&error.code,&message);
            let mut staged_binding=target.binding.clone();staged_binding.pane=staged;
            let kept=if alive {
                tracing::warn!(key,session=name,code=%error.code,reason="destino iniciado mantido vivo; settings privados ficam até a saída do helper","ciclo de vida Claude");
                true
            } else if let Err(cleanup)=super::lifecycle::kill_pane(&io,&staged_binding,&helper,&procs).await {
                tracing::warn!(key,session=name,code=%cleanup.code,reason="destino não confirmado não foi encerrado; settings privados ficam até a saída do helper","ciclo de vida Claude");
                true
            } else {false};
            if kept {
                let retained=recipe.clone();let helper=helper.clone();
                tokio::spawn(async move {
                    let procs=crate::list::procs::SystemProcs::default();
                    loop {
                        match super::lifecycle::process_absent(&procs,helper.pid,helper.birth) {
                            Ok(true)=>break,
                            Ok(false)=>{},
                            Err(error)=>if crate::warn_limit::allow(None,&error.code) {tracing::warn!(code=%error.code,"não consegui conferir a saída do auxiliar da reabertura");},
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    drop(retained);
                });
            }
            tracing::warn!(key,session=name,code=%error.code,reason="reabertura não confirmada; nenhum rollback declarado","ciclo de vida Claude");
        }
        result
    }

    async fn reopen_headless_locked(&self,name:&str,key:&str,generation:u64,id:&str,requested:Option<&str>,from_exit:bool,context:Option<Arc<super::lifecycle::HeadlessOperation>>,attempt:u32)->Result<Value,RuntimeError> {
        use crate::list::procs::ProcessView;
        if context.as_ref().is_some_and(|ctx|ctx.cancelled.load(std::sync::atomic::Ordering::Acquire)) {return Err(failure("lifecycle_cancelled"));}
        let (mut target,handle,saved_recipe,recorded_birth,auto_respawn)= {
            let entries=self.entries.lock().await;
            if entries.iter().any(|(other,e)|other!=key && e.name==name) {return Err(failure("runtime_binding"));}
            match entries.get(key) {
                Some(entry) if entry.name==name && entry.provider=="claude"=> {
                    let EntryHandle::Headless(handle)=&entry.handle else {return Err(failure("lifecycle_provider"));};
                    if context.as_ref().is_some_and(|ctx|entry.generation!=ctx.binding.generation || entry.live.borrow().as_ref().and_then(|s|s.binding.as_ref())!=Some(&ctx.binding) || entry.cano_birth!=Some(ctx.birth)) {return Err(failure("runtime_generation"));}
                    (entry.runtime_target.clone().ok_or_else(||failure("lifecycle_target_unavailable"))?,Some(handle.clone()),entry.relaunch.clone(),entry.cano_birth,entry.auto_respawn)
                },
                None=> {
                    let ctx=context.as_ref().ok_or_else(||failure("runtime_binding"))?;
                    (ctx.target.clone(),None,Some(ctx.recipe.clone()),Some(ctx.birth),ctx.auto)
                },
                _=>return Err(failure("runtime_binding")),
            }
        };
        // Como a abertura: quem troca de conta ou desloga não vê o processo subir no meio.
        let _account=runtime_account_guard("claude",&target.metadata).await?;
        if target.generation!=generation {
            if generation.checked_add(1)!=Some(target.generation) {return Err(failure("runtime_generation"));}
            let reply=handle.as_ref().ok_or_else(||failure("runtime_binding"))?.command(RuntimeCommand {operation_id:id.into(),kind:OperationKind::SetPermissionMode,payload:json!({"mode":"bypassPermissions"})}).await?;
            return if reply.disposition==Disposition::Accepted && reply.payload["reopened"]==true {Ok(reply.payload)} else {Err(failure("runtime_generation"))};
        }
        let view=match &context {Some(ctx)=>ctx.view.clone(),None=>handle.as_ref().ok_or_else(||failure("runtime_binding"))?.view().await?};
        let selected=requested.or_else(||view["permission_mode"].as_str()).ok_or_else(||failure("permission_mode_unknown"))?.to_owned();
        if (!from_exit && view["alive"]!=true) || view["in_progress"]==true || !view["question"].is_null() || view["pending"].as_array().is_none_or(|v|!v.is_empty()) {return Err(failure("lifecycle_busy"));}
        let binding=if let Some(ctx)=&context {ctx.binding.clone()} else {self.interaction_binding_readonly(name).await?};
        if binding.key!=key || binding.generation!=generation {return Err(failure("runtime_binding"));}
        let procs=crate::list::procs::SystemProcs::default();
        let original_birth=recorded_birth.ok_or_else(||failure("lifecycle_process_birth_missing"))?;
        let process=match procs.recipe(i64::from(target.binding.pid)) {
            Ok(process) if process.birth==original_birth && process::liveness(target.binding.pid,key)==process::Liveness::Ours=>Some(process),
            Err(error) if from_exit && error.kind()==std::io::ErrorKind::NotFound && super::lifecycle::process_absent(&procs,i64::from(target.binding.pid),original_birth)?=>None,
            _=>return Err(failure("lifecycle_process_binding")),
        };
        if from_exit && process.is_some() && saved_recipe.as_ref().is_some_and(|recipe| {
            let (pid,birth)=recipe.agent_identity();
            super::lifecycle::process_absent(&procs,pid,birth).is_ok_and(|gone|!gone)
        }) {
            // Caiu a conexão, não a CLI: religa ao mesmo cano em vez de matar o turno vivo.
            self.close_locked(key,generation,false).await?;
            target.metadata["session_id"]=json!(binding.conversation);
            self.open_headless_locked(target.clone(),None,false,true).await?;
            if let Some(entry)=self.entries.lock().await.get_mut(key) {
                entry.auto_respawn=auto_respawn;
                entry.settings_owner=context.as_ref().map(|ctx|ctx.recipe.clone());
            }
            return Ok(json!({"reconnected":true,"generation":generation}));
        }
        let original_sid=target.metadata["session_id"].as_str().ok_or_else(||failure("lifecycle_conversation"))?.to_owned();
        let recipe=if from_exit {saved_recipe.ok_or_else(||failure("relaunch_recipe_missing"))?} else {
            let (agent,birth)=super::lifecycle::launched_agent(&procs,i64::from(target.binding.pid),&original_sid)?;
            let mut recipe=super::claude_relaunch::Recipe::capture(&procs,i64::from(target.binding.pid),agent,birth,&original_sid,false)?;
            if requested==Some("bypassPermissions") && recipe.bypass_enabled()? {return Err(failure("relaunch_bypass_already_enabled"));}
            let config=recipe.config_dir()?;
            if target.metadata["config_dir"].as_str().is_some_and(|p|std::path::Path::new(p)!=config) {return Err(failure("lifecycle_config_changed"));}
            target.metadata["config_dir"]=json!(config);
            if requested==Some("bypassPermissions") {recipe.policy_preflight(&config)?;}
            recipe.preserve_settings(target.state_path.parent().ok_or_else(||failure("relaunch_settings_copy"))?)?;
            Arc::new(recipe)
        };
        let sidecar_dir=super::lifecycle::claude_sidecar_dir()?;
        // A vida nova sai do spec da sessão com a configuração de agora, nunca da receita do processo que
        // morre: a mesma conversa, o modo pedido e o modelo e o esforço que ela usava.
        let prepared={
            let (env,bridge,key,dir,name,runtime)=(self.policy.codex_environment(),self.policy.plugin_bridge(),key.to_owned(),sidecar_dir.clone(),name.to_owned(),tokio::runtime::Handle::current());
            let (sid,mode,model,effort)=(binding.conversation.clone(),selected.clone(),view["model"].clone(),view["effort"].clone());
            let cwd=target.metadata["cwd"].as_str().filter(|cwd|!cwd.is_empty()).ok_or_else(||failure("launch_cwd"))?.to_owned();
            tokio::task::spawn_blocking(move||{
                let sidecar=Value::Object(super::codex_sidecar::load(&super::codex_sidecar::path(&dir,&name)).ok_or_else(||failure("lifecycle_sidecar_read"))?);
                let meta=super::claude_launch::relaunch_meta(sidecar,&sid,&mode,&model,&effort);
                let launch=super::claude_launch::prepare(&env,bridge.as_ref(),&meta,&name,runtime)?;
                let mut extra=serde_json::Map::new();
                if let Ok(stamp)=super::local_policy::config_stamp(meta["config_dir"].as_str().filter(|dir|!dir.is_empty())) {extra.insert("config_marca".into(),json!(stamp));}
                let spec=process::LaunchSpec {provider:process::Provider::Claude,key,cwd:cwd.into(),program:launch.program,env:launch.env.into_iter().collect(),sidecar_dir:dir};
                process::prepare_launch(&spec,extra).map_err(|e|RuntimeError::new(e.code(),"não preparei o lançamento; origem conservada"))
            }).await.map_err(|_|failure("relaunch_recipe"))??
        };
        if !from_exit {recipe.verify(&procs)?;}
        let latest=match &context {Some(ctx)=>ctx.view.clone(),None=>handle.as_ref().ok_or_else(||failure("runtime_binding"))?.view().await?};
        if (!from_exit && latest["alive"]!=true) || latest["in_progress"]==true || !latest["question"].is_null() || latest["pending"].as_array().is_none_or(|v|!v.is_empty())
            || latest["conversation"]!=binding.conversation {return Err(failure("lifecycle_busy"));}
        let phase=format!("claude-reopen:{id}");
        if let Some(handle)=&handle {
            if from_exit {handle.queue(format!("restart-root:{id}"),Action::Prepare {id:id.into(),payload:json!({"operation_id":id,"kind":"restart","payload":{}}),entry_id:None}).await?;}
            handle.queue(format!("{phase}:prepare"),Action::Prepare {id:phase.clone(),payload:json!({"logical_id":id,"kind":"claude_reopen","conversation":binding.conversation,"generation":generation}),entry_id:None}).await?;
            self.close_locked(key,generation,false).await?;
        }
        let lease=wait_lease(&target.lease_path).await?;
        let mut store=Store::open(&target.state_path,&target.projection_dir,QueueState::new(key,generation,name,vec![])).map_err(|_|failure("queue_io"))?;
        let cancelled=context.as_ref().is_some_and(|ctx|ctx.cancelled.load(std::sync::atomic::Ordering::Acquire));
        let (current_generation,next_generation)=super::lifecycle::retry_fence(&binding,store.state(),id,attempt,cancelled,None)?;
        let intent=json!({"operation_id":id,"key":key,"conversation":binding.conversation,"from_generation":generation,"target_generation":next_generation});
        store.exec(current_generation,&format!("{phase}:intent:{attempt}"),ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())},Action::AdvanceLifecycle {generation:current_generation,state:json!({"claude_lifecycle_intent":intent})}).map_err(|_|failure("queue_io"))?;
        if let Some(meta)=store.state().runtime_state.get("metadata").filter(|meta|meta["key"]==key) {target.metadata=meta.clone();}
        if target.metadata["session_id"]!=binding.conversation {return Err(failure("lifecycle_conversation"));}
        let sidecar=super::lifecycle::claude_sidecar_dir()?.join(format!("{name}.json"));
        let persisted:Value=serde_json::from_slice(&super::queue::read_replaced(&sidecar).map_err(|_|failure("lifecycle_sidecar_read"))?).map_err(|_|failure("lifecycle_sidecar_read"))?;
        if persisted["key"]!=key || persisted["session_id"]!=binding.conversation {return Err(failure("lifecycle_conversation"));}
        let sidecar_expected_pid=if persisted["cano"].is_null() {0} else {persisted["cano"]["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or_else(||failure("lifecycle_sidecar_read"))?};
        if attempt>0 && persisted["cano"]["pid"].as_u64().and_then(|p|u32::try_from(p).ok()).is_some_and(|pid|process::liveness(pid,key)==process::Liveness::Ours) {return Err(failure("relaunch_previous_still_alive"));}
        let step=format!("{phase}:attempt:{attempt}");
        let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
        store.exec(current_generation,&format!("{step}:prepare"),clock,Action::Prepare {id:step.clone(),payload:json!({"logical_id":id,"kind":"claude_spawn_attempt","attempt":attempt}),entry_id:None}).map_err(|_|failure("queue_io"))?;
        store.exec(current_generation,&format!("{step}:dispatch"),clock,Action::BeginDispatch {id:step.clone(),wire_id:step.clone(),staged:false}).map_err(|_|failure("queue_io"))?;
        if let Some(process)=&process && procs.recipe(process.pid).map_err(|_|failure("lifecycle_process_read"))?!=*process {return Err(failure("lifecycle_process_changed"));}
        let original=process::Cano {pid:target.binding.pid,escuta:target.binding.escuta.clone(),token:target.binding.token.clone(),ts:original_birth,versao:target.binding.versao,extra:Default::default()};
        process::kill(&original,key,&sidecar_dir).await.map_err(|e|RuntimeError::new(e.code(),"o cano original não foi encerrado"))?;
        if context.as_ref().is_some_and(|ctx|ctx.cancelled.load(std::sync::atomic::Ordering::Acquire)) {return Err(failure("lifecycle_cancelled"));}
        let started=process::spawn_recipe(prepared).await;
        let cano=match started {
            Ok(cano)=>cano,
            Err(error)=> {
                let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
                let mut failed_target=target.clone();failed_target.binding.pid=sidecar_expected_pid;
                if super::lifecycle::patch_sidecar(&failed_target,&json!({"cano":null}))?["updated"]!=true {return Err(failure("lifecycle_sidecar_changed"));}
                store.exec(current_generation,&format!("{step}:failed"),clock,Action::Finish {id:step.clone(),status:super::queue::Status::Unknown,result:json!({"error_code":error.code(),"alive":false})}).map_err(|_|failure("queue_io"))?;
                let error=RuntimeError::new(error.code(),"a sessão não reabriu e ficou sem processo; nenhum modo foi confirmado");
                super::actor::mark_live_error(&self.live_sender(name),&error.code,&error.message);return Err(error);
            },
        };
        recipe.pin_settings(i64::from(cano.pid),cano.extra["process_birth"].as_f64().ok_or_else(||failure("lifecycle_process_birth_missing"))?)?;
        let mut launched_guard=process::RelaunchGuard::new(&cano,key,&sidecar_dir,recipe.clone());
        if context.as_ref().is_some_and(|ctx|ctx.cancelled.load(std::sync::atomic::Ordering::Acquire)) {return Err(failure("lifecycle_cancelled"));}
        if self.entries.lock().await.iter().any(|(other,entry)|entry.name==name && other!=key) {return Err(failure("runtime_binding"));}
        target.metadata["cano"]=serde_json::to_value(&cano).map_err(|_|failure("cano_json"))?;
        target.metadata["permission_mode"]=Value::Null;
        target.metadata["lifecycle_pending"]=json!(true);
        target.metadata["initialized"]=json!(false);
        target.metadata["runtime_counter"]=json!(view["runtime_counter"].as_u64().unwrap_or(0).max(store.state().runtime_state["view"]["runtime_counter"].as_u64().unwrap_or(0)));
        target.metadata["previous_non_plan"]=view["previous_non_plan"].clone();
        target.metadata["session_id"]=json!(binding.conversation);
        target.generation=next_generation;
        target.binding=CanoBinding {pid:cano.pid,escuta:cano.escuta.clone(),token:cano.token.clone(),versao:cano.versao};
        let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
        store.exec(current_generation,&format!("{step}:advance"),clock,Action::AdvanceLifecycle {generation:target.generation,state:json!({"metadata":target.metadata})}).map_err(|_|failure("queue_io"))?;
        let mut original_target=target.clone();original_target.generation=generation;original_target.metadata["cano"]=serde_json::to_value(&original).unwrap();
        original_target.binding=CanoBinding {pid:sidecar_expected_pid,escuta:original.escuta.clone(),token:original.token.clone(),versao:original.versao};
        if super::lifecycle::patch_sidecar(&original_target,&json!({"cano":serde_json::to_value(&cano).unwrap()}))?["updated"]!=true {return Err(failure("lifecycle_sidecar_changed"));}
        drop(store);drop(lease);
        let result=async {
            self.open_headless_locked(target.clone(),None,false,false).await?;
            if let Some(entry)=self.entries.lock().await.get_mut(key) {
                let fresh=entry.relaunch.as_ref().ok_or_else(||failure("relaunch_recipe_missing"))?;
                let (pid,birth)=fresh.agent_identity();recipe.pin_settings(pid,birth)?;
                entry.settings_owner=Some(recipe.clone());entry.auto_respawn=auto_respawn;
            }
            let handle=self.entry(key,target.generation).await?;
            let deadline=tokio::time::Instant::now()+super::claude_reopen::STARTUP_WAIT;
            loop {
                let snapshot=handle.snapshot().await?;
                if !snapshot["error"].is_null() || snapshot["view"]["alive"]==false {return Err(failure("relaunch_initialize"));}
                if snapshot["view"]["initialized"]==true && snapshot["view"]["conversation"]==binding.conversation {break;}
                if tokio::time::Instant::now()>=deadline {return Err(failure("relaunch_initialize_timeout"));}
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let confirmed=handle.command(RuntimeCommand {operation_id:format!("boot-confirm:{id}:{attempt}"),kind:OperationKind::SetPermissionMode,payload:json!({"mode":selected})}).await?;
            if confirmed.disposition!=Disposition::Accepted {return Err(failure("relaunch_permission_unconfirmed"));}
            let view=handle.view().await?;
            if view["permission_mode"]!=selected || view["conversation"]!=binding.conversation {return Err(failure("relaunch_permission_unconfirmed"));}
            let EntryHandle::Headless(actor)=&handle else {return Err(failure("lifecycle_provider"));};
            actor.lifecycle_ready(crate::interactions::InteractionBinding {key:key.into(),conversation:binding.conversation.clone(),generation:target.generation},format!("boot-confirm:{id}:{attempt}"),selected.clone()).await?;
            let payload=json!({"mode":selected,"current":selected,"previous_non_plan":view["previous_non_plan"],"reopened":true});
            handle.queue(format!("{phase}:root-finish"),Action::Finish {id:id.into(),status:super::queue::Status::Accepted,result:serde_json::to_value(RuntimeReply {operation_id:id.into(),disposition:Disposition::Accepted,payload:payload.clone()}).unwrap()}).await?;
            handle.queue(format!("{phase}:finish"),Action::Finish {id:phase.clone(),status:super::queue::Status::Accepted,result:payload.clone()}).await?;
            launched_guard.disarm();
            Ok(payload)
        }.await;
        if let Err(error)=&result {
            let stopped=self.close_locked(key,target.generation,false).await;
            if let Err(stop)=stopped {tracing::warn!(key,code=%stop.code,"ator do relançamento não foi encerrado");}
            if let Err(stop)=process::kill(&cano,key,&sidecar_dir).await {tracing::warn!(key,code=stop.code(),"cano não confirmado não foi encerrado");}
            super::actor::mark_live_error(&self.live_sender(name),&error.code,&error.message);
            // Como o Python: o bypass pedido que não pegou (a CLI recusa o `initialize`) volta a conversa ao
            // modo de antes num processo novo, em vez de deixá-la parada até alguém reabrir à mão.
            if requested==Some("bypassPermissions") && !from_exit && let Some(previous)=view["permission_mode"].as_str().filter(|mode|*mode!="bypassPermissions") {
                let context=Arc::new(super::lifecycle::HeadlessOperation {target:target.clone(),
                    binding:crate::interactions::InteractionBinding {key:key.into(),conversation:binding.conversation.clone(),generation:target.generation},
                    recipe:recipe.clone(),view:view.clone(),birth:cano.extra["process_birth"].as_f64().unwrap_or(0.0),auto:auto_respawn,
                    cancelled:Arc::new(std::sync::atomic::AtomicBool::new(false))});
                let restored=Box::pin(self.reopen_headless_locked(name,key,target.generation,&format!("{id}:restore"),Some(previous),true,Some(context),0)).await;
                return Err(match restored {
                    Ok(_)=>RuntimeError::new("erro_permissao_reabrir",&format!("a sessão não reabriu em bypass ({}) e voltou ao modo de antes",error.code)),
                    Err(restore)=>RuntimeError::new(&error.code,&format!("a sessão não reabriu em bypass e não voltou ao modo de antes ({}); ela ficou sem processo",restore.code)),
                });
            }
        }
        result
    }

    /// Callback do Monitor: a ausência só encerra o vínculo que ele estava observando.
    pub async fn dead(&self,name:&str,key:&str,conversation:&str,generation:u64)->Result<crate::state::facts::Dead,RuntimeError> {
        use crate::state::facts::Dead;
        let barrier=self.barrier(key).await;let _guard=barrier.lock().await;
        let (terminal,headless,closed)={
            let entries=self.entries.lock().await;
            if entries.iter().any(|(other,entry)|entry.name==name && (other!=key || entry.generation!=generation)) {return Ok(Dead::InTransfer);}
            match entries.get(key) {
                Some(entry) if entry.name==name && entry.generation==generation=>match &entry.handle {
                    EntryHandle::Terminal {target,..} if target.binding.conversation==conversation=>(Some(target.clone()),None,None),
                    EntryHandle::Headless(_) if entry.provider=="claude"=> {
                        let actual=entry.live.borrow().as_ref().and_then(|state|state.binding.clone()).ok_or_else(||failure("lifecycle_binding_unavailable"))?;
                        if !super::lifecycle::matches_life(&actual,key,conversation,generation) {return Ok(Dead::InTransfer);}
                        let target=entry.runtime_target.clone().ok_or_else(||failure("lifecycle_target_unavailable"))?;
                        let closed=super::lifecycle::ClosedLife {name:name.into(),binding:actual,pid:i64::from(target.binding.pid),birth:entry.cano_birth.ok_or_else(||failure("lifecycle_process_birth_missing"))?,state_path:target.state_path.clone(),lease_path:target.lease_path.clone(),projection:target.projection_dir.clone()};
                        (None,Some(target),Some(closed))
                    },
                    _=>return Ok(Dead::InTransfer),
                },
                None=>(None,None,barrier.closed.lock().map_err(|_|failure("lifecycle_closed_record"))?.clone()),
                _=>return Ok(Dead::InTransfer),
            }
        };
        let _ingress=super::lifecycle::ClosedIngress::acquire(&self.ingress,name).await?;
        let procs=crate::list::procs::SystemProcs::default();
        if terminal.is_none() {
            let closed=closed.ok_or_else(||failure("state_lifecycle_binding_missing"))?;
            if closed.name!=name || !super::lifecycle::matches_life(&closed.binding,key,conversation,generation) {return Ok(Dead::InTransfer);}
            if !super::lifecycle::process_absent(&procs,closed.pid,closed.birth)? {return Ok(Dead::InTransfer);}
            let sidecar=super::lifecycle::claude_sidecar_dir()?.join(format!("{name}.json"));
            match super::queue::read_replaced(&sidecar) {
                Ok(bytes)=> {let _:Value=serde_json::from_slice(&bytes).map_err(|_|failure("lifecycle_sidecar_read"))?;return Ok(Dead::InTransfer);},
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(_)=>return Err(failure("lifecycle_sidecar_read")),
            }
            if headless.is_some() {self.close_locked(key,generation,false).await?;}
            let lease=wait_lease(&closed.lease_path).await?;
            let mut store=Store::open(&closed.state_path,&closed.projection,QueueState::new(key,generation,name,vec![])).map_err(|_|failure("queue_io"))?;
            if store.state().owner_key!=key || store.state().generation!=generation {return Ok(Dead::InTransfer);}
            let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
            store.exec(generation,&format!("dead-clear:{key}:{generation}"),clock,Action::Clear).map_err(|_|failure("queue_io"))?;
            if let Some(interactions)=&self.interactions {interactions.forget_binding(&closed.binding).map_err(crate::interactions::runtime_error)?;}
            self.forget_claude_rate(&closed.binding)?;
            if let Some(plugin)=&self.plugin {plugin.forget(key).map_err(|error|failure(error.0))?;}
            drop(store);drop(lease);
            return Ok(Dead::Ok);
        }
        let target=terminal.unwrap();
        // Tabela lida com sucesso e pid original ausente: erro de captura não equivale a morte.
        if !super::lifecycle::process_absent(&procs,i64::from(target.metadata.agent_pid),target.metadata.agent_birth)? {return Ok(Dead::InTransfer);}
        let io=crate::terminal_input::ProcessIo::default();
        // Pane morto com a sessão do multiplexador de pé (`remain-on-exit`): a fila fica para depois do
        // `/resume`, como no Python; só a sessão que sumiu leva a fila junto.
        let gone=match super::lifecycle::pane(&io,&target.binding).await {
            Ok(pane) if !pane.dead=>return Ok(Dead::InTransfer),
            Ok(_)=>false,
            Err(error)=> {
                // Se a sessão sumiu, a lista do multiplexador deve provar isso; silêncio não prova.
                use crate::terminal_input::{TerminalIo,CommandRequest};
                let (program,prefix)=target.binding.mux_argv.split_first().ok_or_else(||failure("lifecycle_mux"))?;
                let mut args=prefix.to_vec();args.extend(["list-sessions".into(),"-F".into(),"#{session_name}\t#{session_created}".into()]);
                let list=io.command(CommandRequest {program:program.clone(),args,stdin:vec![]}).await.map_err(|_|failure("lifecycle_mux"))?;
                if !list.success {return Err(error);}
                let text=std::str::from_utf8(&list.stdout).map_err(|_|failure("lifecycle_capture"))?;
                if session_of_life(text,name,target.binding.created) {return Err(error);}
                true
            },
        };
        // A chave/geração ficou presa pela barreira; callbacks de A nunca chegam ao Store de B.
        self.close_locked(key,generation,false).await?;
        let lease=wait_lease(&target.lease_path).await?;
        let target=target.clone();
        tokio::task::spawn_blocking(move || {
            let _lease=lease;
            let mut store=Store::open(&target.state_path,&target.projection_dir,QueueState::new(&target.key,target.generation,&target.name,vec![])).map_err(|_|failure("queue_io"))?;
            if store.state().owner_key!=target.key || store.state().generation!=generation {return Err(failure("runtime_generation"));}
            let clock=ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
            if gone {store.exec(generation,&format!("dead-clear:{key}:{generation}",key=target.key),clock,Action::Clear).map_err(|_|failure("queue_io"))?;}
            store.exec(generation,&format!("dead-state:{key}:{generation}",key=target.key),clock,Action::AdvanceLifecycle {generation,state:json!({"terminal_lifecycle":{"closed":true,"conversation":target.binding.conversation}})}).map_err(|_|failure("queue_io"))?;
            Ok::<_,RuntimeError>(())
        }).await.map_err(|_|failure("lifecycle_worker"))??;
        Ok(Dead::Ok)
    }

    /// Sessão fora do Rust: o feed mostra a parada (`None`), e o canal sem receptor sai do mapa.
    async fn clear_live(&self,name:&str) {
        // Outra vida com o mesmo nome (reaberta antes deste fechamento) segue dona do canal.
        if self.entries.lock().await.values().any(|entry|entry.name == name) { return; }
        {
            let mut map=self.terminal_status.lock().unwrap_or_else(|e|e.into_inner());
            // Cópia fora do mapa é o ator de outra vida com o mesmo nome, que nasce antes de entrar em
            // `entries`: zerar ou tirar o canal dele deixaria o monitor sem status.
            if let Some(sender)=map.get(name) && sender.sender_count()==1 {
                let failed=sender.borrow().error.is_some();
                if !failed {
                    sender.send_replace(Default::default());
                    if sender.receiver_count()==0 { map.remove(name); }
                }
            }
        }
        let mut map = self.live.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(sender) = map.get(name) && sender.sender_count() == 1 {
            // Vida que acabou com erro: o problema fica até a próxima abertura, que escreve por cima.
            // ponytail: o canal com erro de sessão apagada fica no mapa; um por nome, sem crescer.
            if sender.borrow().as_ref().is_some_and(|state|state.error.is_some()) { return; }
            sender.send_replace(None);
            if sender.receiver_count() == 0 { map.remove(name); }
        }
    }
    async fn barrier(&self,key:&str) -> Arc<super::lifecycle::Barrier> {
        self.lifecycle.lock().await.entry(key.into()).or_default().clone()
    }
    /// Chave do ator de entrada terminal da sessão `name` e o retrato dele; `None` sem ator aberto.
    pub async fn terminal_view(&self,name:&str) -> Option<(String,Value)> {
        let found = self.entries.lock().await.iter().find_map(|(key,entry)| match &entry.handle {
            EntryHandle::Terminal {target,handle} if target.name==name=>Some((key.clone(),handle.clone())),
            _=>None,
        });
        let (key,handle) = found?;
        match handle.snapshot().await {
            Ok(view)=>Some((key,view)),
            Err(error)=>Some((key,json!({"error":error.code,"view":{}}))),
        }
    }
    /// Entrada aberta da sessão `name`, para escrever. Chamar depois do `ingress().enter`: a procurada
    /// antes de esperar a porta pode ser a que o relançamento parou.
    /// `healthy` é a negação do `_rust_failed` do Python; `terminal_facts` conta como doente aqui
    /// (vínculo trocado, só o `prepare_session` refaz), `receipt_scan` não.
    pub async fn writable(&self,name:&str) -> Option<WriteTarget> {
        // Nome repetido é defeito de quem abriu a entrada; a escrita vai para a geração mais nova.
        let (key,generation,provider,handle) = self.entries.lock().await.iter().filter(|(_,e)|e.name==name).max_by_key(|(_,e)|e.generation)
            .map(|(key,e)|(key.clone(),e.generation,e.provider.clone(),e.handle.clone()))?;
        let terminal = handle.has_terminal();
        let healthy = probe(&handle).await.unwrap_or(false);
        Some(WriteTarget { key,generation,provider,terminal,healthy,handle })
    }
    /// A saúde que o painel da fila mostra: `None` quando o ator não respondeu no prazo (ocupado entregando),
    /// para quem pergunta manter o último valor em vez de piscar os botões a cada entrega.
    pub async fn write_health(&self,name:&str) -> Option<bool> {
        let handle = self.entries.lock().await.values().filter(|e|e.name==name).max_by_key(|e|e.generation).map(|e|e.handle.clone());
        match handle { Some(handle)=>probe(&handle).await, None=>Some(false) }
    }
    /// A entrada mais nova do nome é de terminal? Sem retrato: serve a decisão que vem antes da porta.
    pub async fn is_terminal(&self,name:&str) -> bool {
        self.entries.lock().await.values().filter(|e|e.name==name).max_by_key(|e|e.generation)
            .is_some_and(|e|e.handle.has_terminal())
    }
    /// Leitura pura: quem exclui já pode segurar exclusividade.
    pub async fn account_usage(&self, key: &crate::accounts::AccountKey) -> crate::accounts::UsageFacts {
        let entries = self.entries.lock().await;
        let sessions = entries.values().filter(|entry| entry.account_key.as_ref().is_some_and(|account| account == key))
            .map(|entry| entry.name.clone()).collect();
        crate::accounts::UsageFacts { complete: true, sessions, ..Default::default() }
    }

    pub async fn snapshots(&self) -> Result<Vec<RuntimeEvent>,RuntimeError> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.generation,entry.handle.clone())).collect();
        let mut output = Vec::new();
        for (key,generation,handle) in entries {
            // Uma sessão sem ator não tira o retrato das outras: ela fica de fora até ser liberada.
            match handle.snapshot().await {
                Ok(data)=>output.push(RuntimeEvent { key,generation,revision:data["revision"].as_u64().unwrap_or(0),channel:"snapshot".into(),data }),
                Err(error)=>if crate::warn_limit::allow(Some(&key),&error.code) { tracing::warn!(key,code=%error.code,"sessão fora do retrato inicial dos eventos") },
            }
        }
        Ok(output)
    }
    /// Retrato de todas as sessões, por chave, para a lista. Diferente de `snapshots`, a sessão cujo
    /// ator não respondeu fica com `{"error": código}`: fora do retrato ela pareceria parada.
    pub async fn list_snapshots(&self) -> BTreeMap<String,Value> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.handle.clone())).collect();
        let asks = entries.into_iter().map(|(key,handle)| async move {
            let data = match tokio::time::timeout(Duration::from_secs(1),handle.snapshot()).await {
                Ok(Ok(data))=>data,
                Ok(Err(error))=>json!({"error":error.code}),
                Err(_)=>json!({"error":"runtime_snapshot_timeout"}),
            };
            (key,data)
        });
        futures_util::future::join_all(asks).await.into_iter().collect()
    }
    pub async fn list_snapshots_readonly(&self) -> BTreeMap<String,Value> {
        let entries=self.entries.lock().await;
        entries.iter().map(|(key,entry)| {
            let unavailable=||json!({"key":key,"generation":entry.generation,"error":"runtime_live_unavailable"});
            let data=match &entry.handle {
                EntryHandle::Terminal {target,..} if target.key==*key && target.generation==entry.generation=>{
                    match self.terminal_states.as_ref().and_then(|states|states.get(&entry.name,Some(&target.binding.conversation))) {
                        Some(state)=>json!({"key":key,"generation":entry.generation,"error":state.problema,"view":{"alive":state.state!="dead","conversation":target.binding.conversation,"headless":false,"public_state":&*state}}),
                        None=>unavailable(),
                    }
                },
                EntryHandle::Terminal {..}=>unavailable(),
                _=>match entry.live.borrow().clone() {
                    Some(live) if live.binding.as_ref().is_some_and(|binding|binding.key==*key && binding.generation==entry.generation && !binding.conversation.is_empty()) && live.public_state["state"].is_string()=>{
                        json!({"key":key,"generation":entry.generation,"error":live.error.as_ref().map(|error|&error.0),"view":{"alive":live.public_state["state"]!="dead","conversation":live.binding.as_ref().map(|binding|&binding.conversation),"headless":matches!(entry.handle,EntryHandle::Headless(_)),"public_state":live.public_state}})
                    },
                    _=>unavailable(),
                },
            };(key.clone(),data)
        }).collect()
    }
    pub async fn shutdown(&self) -> Result<(),RuntimeError> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.generation)).collect();
        for (key,generation) in entries { self.close(&key,generation).await?; }
        Ok(())
    }
}

#[derive(Clone)]
struct Gateway { registry:Arc<RuntimeRegistry>,secret:String,instance:String,protocol:u32 }

pub fn startup_line(protocol:u32,instance:&str,port:u16) -> String {
    json!({"type":"runtime_ready","protocol":protocol,"instance":instance,"port":port}).to_string()
}

pub async fn serve(listener:tokio::net::TcpListener,registry:Arc<RuntimeRegistry>,secret:String,instance:String,protocol:u32) -> std::io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"IPC somente no loopback")); }
    let state = Gateway { registry,secret,instance,protocol };
    let router = Router::new().route("/runtime/op",post(operation)).route("/runtime/events",get(events))
        .layer(middleware::from_fn_with_state(state.clone(),authorize)).with_state(state);
    axum::serve(axum::serve::ListenerExt::tap_io(listener,crate::nodelay),router.into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn authorize(State(state):State<Gateway>,request:Request,next:Next) -> Response {
    let local = request.extensions().get::<ConnectInfo<SocketAddr>>().is_some_and(|info|info.0.ip().is_loopback());
    let secret = request.headers().get("x-hangar-internal").map(|h|h.as_bytes()).unwrap_or(&[]);
    let instance = request.headers().get("x-hangar-runtime-instance").map(|h|h.as_bytes()).unwrap_or(&[]);
    if !local || secret.ct_eq(state.secret.as_bytes()).unwrap_u8() != 1 || instance.ct_eq(state.instance.as_bytes()).unwrap_u8() != 1 {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol:u32,instance:String,key:String,generation:u64,operation_id:String,clock:ClockSample,command:Value,
}

async fn operation(State(state):State<Gateway>,request:Request) -> Response {
    let body = match to_bytes(request.into_body(),MAX_ENVELOPE).await { Ok(body)=>body,Err(_)=>return refuse(StatusCode::PAYLOAD_TOO_LARGE,None,"body") };
    let envelope:Envelope = match serde_json::from_slice(&body) { Ok(envelope)=>envelope,Err(_)=>return refuse(StatusCode::BAD_REQUEST,None,"envelope") };
    let _ = envelope.clock;
    let mismatch = if envelope.protocol != state.protocol { Some("protocol") } else if envelope.instance != state.instance { Some("instance") }
        else if envelope.operation_id.is_empty() { Some("operation_id") } else { None };
    if let Some(check) = mismatch { return refuse(StatusCode::CONFLICT,Some(&envelope.key),check); }
    let result = dispatch(&state.registry,&envelope).await;
    match result {
        Ok(result)=>json_response(StatusCode::OK,json!({"ok":true,"result":result})),
        Err(error)=>{
            if crate::warn_limit::allow(Some(&envelope.key),&error.code) {
                // Só o nome vem do descritor: o resto dele é caminho e credencial do cano.
                let name = envelope.command["descriptor"]["name"].as_str().unwrap_or("");
                tracing::warn!(key=%envelope.key,session=%name,kind=%envelope.command["kind"].as_str().unwrap_or("?"),
                    code=%error.code,reason=%error.message,"runtime recusou operação");
            }
            json_response(StatusCode::SERVICE_UNAVAILABLE,json!({"ok":false,"error_code":error.code,"message":error.message}))
        }
    }
}

/// Recusa antes do despacho: diz qual conferência falhou, sem o corpo.
fn refuse(status:StatusCode,key:Option<&str>,check:&'static str) -> Response {
    if crate::warn_limit::allow(key,check) {
        tracing::warn!(status=status.as_u16(),key=%key.unwrap_or(""),check=%check,"runtime recusou envelope");
    }
    status.into_response()
}

fn same_terminal_life(old:&super::terminal::TerminalTarget,new:&super::terminal::TerminalTarget) -> bool {
    old.key==new.key && old.name==new.name && old.binding.pane==new.binding.pane && old.binding.created==new.binding.created
        && old.binding.conversation==new.binding.conversation && old.binding.generation==new.binding.generation
        && old.metadata.agent_pid==new.metadata.agent_pid && crate::terminal_facts::same_birth(old.metadata.agent_birth,new.metadata.agent_birth)
        && old.transcript==new.transcript && old.state_path==new.state_path && old.projection_dir==new.projection_dir && old.lease_path==new.lease_path
}

fn json_response(status:StatusCode,value:Value) -> Response {
    (status,[("content-type","application/json")],value.to_string()).into_response()
}

async fn dispatch(registry:&RuntimeRegistry,envelope:&Envelope) -> Result<Value,RuntimeError> {
    let command = &envelope.command;
    let kind = command["kind"].as_str().ok_or_else(||failure("command_kind"))?;
    let fields:&[&str] = match kind {
        "open"=>&["kind","descriptor","launch"],
        "submit"=>&["kind","text","steer","pre_transcript"],
        "control"=>&["kind","control","payload"],
        "queue"=>&["kind","action"],
        "ingress"=>&["kind","name","closed","held"],
        "close"=>&["kind","kill"],
        "snapshot" | "drain" | "confirm" | "ensure_projection"=>&["kind"],
        _=>return Err(failure("command_kind")),
    };
    if !command.as_object().is_some_and(|object|object.keys().all(|key|fields.contains(&key.as_str()))) {
        return Err(failure("command_fields"));
    }
    if kind == "ingress" {
        let name = command["name"].as_str().filter(|n|!n.is_empty()).ok_or_else(||failure("ingress_payload"))?;
        let closed = command["closed"].as_bool().ok_or_else(||failure("ingress_payload"))?;
        // `held`: fechamento da troca de conversa; a reabertura correspondente leva o mesmo `held`.
        let held = match &command["held"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("ingress_payload"))? };
        let gates = registry.ingress();
        if closed {registry.cancel_lifecycle_name(name).await;}
        match (closed,held) {
            (true,false)=>gates.close(name,INGRESS_CLOSE_WAIT).await.map_err(|_|failure("ingress_busy"))?,
            (true,true)=>gates.hold(name,INGRESS_CLOSE_WAIT).await.map_err(|_|failure("ingress_busy"))?,
            (false,false)=>gates.open(name),
            (false,true)=>gates.release(name),
        }
        return Ok(json!({"closed":closed}));
    }
    if kind == "open" {
        // `launch`: quem abre pode subir o processo (só sem terminal, e só se o gravado não for nosso).
        let launch = match &command["launch"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("open_launch"))? };
        let mut value=command["descriptor"].clone();
        if value["provider"]=="claude" && value["headless"]==false {
            let parsed:Descriptor=serde_json::from_value(value.clone()).map_err(|_|failure("descriptor_shape"))?;
            let binding=terminal_binding(&parsed)?;
            let metadata=crate::terminal_facts::TerminalMetadata::resolve(&parsed.meta,&binding,std::path::Path::new(&parsed.jsonl)).await.map_err(|error|failure(error.0))?;
            value["meta"]["config_dir"]=json!(metadata.config_dir);value["meta"]["agent_pid"]=json!(metadata.agent_pid);
            value["meta"]["agent_birth"]=json!(metadata.agent_birth);value["meta"]["fingerprint"]=json!(metadata.fingerprint);value["meta"]["message_origin"]=json!(metadata.origin);
        }
        return match descriptor(&value,launch)? {
            Target::Headless(target,sidecar_dir) if launch=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                let sidecar_dir = sidecar_dir.filter(|dir|!dir.as_os_str().is_empty()).ok_or_else(||failure("launch_sidecar_dir"))?;
                registry.open_with_launch(target,sidecar_dir).await
            },
            // Sem terminal com a pasta do arquivo: o Rust administra o processo mesmo sem subir agora (o
            // Claude que cai é religado aqui).
            Target::Headless(target,Some(sidecar_dir)) if !sidecar_dir.as_os_str().is_empty()=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open_managed(target,sidecar_dir).await
            },
            // Codex sem a pasta do sidecar gravaria o arquivo da sessão pelo Python: recusa.
            Target::Headless(target,_) if target.provider == "codex"=>Err(failure("launch_sidecar_dir")),
            Target::Headless(target,_)=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open(target).await
            },
            Target::Terminal(_) if launch=>Err(failure("open_launch")),
            Target::Terminal(target)=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open_terminal(target).await
            }
        };
    }
    if kind == "close" {
        let kill = match &command["kill"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("close_kill"))? };
        return if kill { registry.close_with_kill(&envelope.key,envelope.generation).await } else { registry.close(&envelope.key,envelope.generation).await };
    }
    // Envio, fila e controle seguram a leitura da trava de estacionar: o processo não morre no meio deles, e a
    // sessão estacionada é reaberta pelo arquivo dela antes de atender.
    let resumable = matches!(kind,"submit"|"queue"|"control");
    let barrier = registry.barrier(&envelope.key).await;
    let _parking = if resumable { Some(barrier.park.read().await) } else { None };
    let handle = match registry.entry(&envelope.key,envelope.generation).await {
        Err(missing) if resumable => {
            let parked = barrier.parked.lock().map_err(|_|failure("lifecycle_parked"))?.clone();
            match parked { Some(name) => match registry.open_claude_headless(&name,true).await {
                Some(opened) => { opened?; registry.entry(&envelope.key,envelope.generation).await? },
                None => return Err(missing),
            }, None => return Err(missing) }
        },
        found => found?,
    };
    match kind {
        "submit"=> {
            // Só envios: as consultas que o Python repete (confirm, snapshot) inflariam a conta.
            crate::migration_status::count_private(if matches!(&handle,EntryHandle::Terminal {..}) {"send_terminal"} else {"send_headless"});
            if matches!(&handle,EntryHandle::Terminal {..}) && (command.get("steer").is_some_and(|value|!value.is_boolean())
                || command.get("pre_transcript").is_some_and(|value|!value.is_boolean())) {return Err(failure("terminal_payload"));}
            let kind = if command["steer"] == true && !matches!(&handle,EntryHandle::Terminal {..}) { OperationKind::Steer } else { OperationKind::Input };
            let reply = handle.command(RuntimeCommand { operation_id:envelope.operation_id.clone(),kind,
                payload:json!({"text":command["text"],"pre_transcript":command["pre_transcript"].as_bool().unwrap_or(false)}) }).await?;
            serde_json::to_value(reply).map_err(|_|failure("reply_json"))
        }
        "control"=> {
            if let EntryHandle::Terminal {handle,..}=&handle {
                let control=command["control"].as_str().ok_or_else(||failure("control_kind"))?;
                let reply=handle.control(envelope.operation_id.clone(),control.into(),command["payload"].clone()).await?;
                return serde_json::to_value(reply).map_err(|_|failure("reply_json"));
            }
            let control:OperationKind = serde_json::from_value(command["control"].clone()).map_err(|_|failure("control_kind"))?;
            let reply = handle.command(RuntimeCommand { operation_id:envelope.operation_id.clone(),kind:control,payload:command["payload"].clone() }).await?;
            serde_json::to_value(reply).map_err(|_|failure("reply_json"))
        }
        "queue"=>handle.queue(envelope.operation_id.clone(),serde_json::from_value::<Action>(command["action"].clone()).map_err(|_|failure("queue_action"))?).await,
        "snapshot"=>handle.snapshot().await,
        "drain"=>handle.drain().await,
        "confirm"=>handle.confirm().await,
        "ensure_projection"=>handle.ensure_projection().await,
        _=>Err(failure("command_kind")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    name:String,key:String,provider:String,headless:bool,meta:Value,jsonl:String,
    projection_dir:std::path::PathBuf,state_path:std::path::PathBuf,lock_path:std::path::PathBuf,generation:u64,
    /// Pasta do arquivo da sessão, onde o cano subido pelo Rust põe socket e log; só no `open` com `launch`.
    #[serde(default)] sidecar_dir:Option<std::path::PathBuf>,
}

/// Sem terminal leva a pasta do arquivo da sessão (`sidecar_dir`), que só o `open` com `launch` usa. O
/// Codex com terminal não abre por aqui: quem o adota é a vigia do Rust.
enum Target {Headless(RuntimeTarget,Option<std::path::PathBuf>),Terminal(super::terminal::TerminalTarget)}

/// Vínculo do pane do Claude com terminal.
fn terminal_binding(descriptor:&Descriptor) -> Result<crate::terminal_input::TerminalBinding,RuntimeError> {
    let binding:crate::terminal_input::TerminalBinding=serde_json::from_value(descriptor.meta["terminal"].clone()).map_err(|_|failure("terminal_binding"))?;
    if binding.generation!=descriptor.generation || binding.name!=descriptor.name || binding.conversation.is_empty() || binding.mux_argv.is_empty()
        || binding.mux_argv.iter().any(|v|v.contains('\0'))
        || (!binding.windows && (!binding.pane.starts_with('%') || binding.pane[1..].parse::<u64>().is_err()))
        || (binding.windows && !binding.pane.starts_with(&format!("={}:",binding.name))) {return Err(failure("terminal_binding"));}
    Ok(binding)
}

fn descriptor(value:&Value,launch:bool) -> Result<Target,RuntimeError> {
    let descriptor:Descriptor = serde_json::from_value(value.clone()).map_err(|_|failure("descriptor_shape"))?;
    if descriptor.meta["key"] != descriptor.key || descriptor.key.is_empty() { return Err(failure("descriptor_binding")); }
    if !descriptor.headless {
        if descriptor.meta.get("cano").is_some(){return Err(failure("descriptor_provider"));}
        if descriptor.provider!="claude" {return Err(failure("descriptor_provider"));}
        let binding = terminal_binding(&descriptor)?;
        if descriptor.jsonl.is_empty() {return Err(failure("terminal_binding"));}
        let metadata=crate::terminal_facts::TerminalMetadata::from_descriptor(&descriptor.meta).map_err(|error|failure(error.0))?;
        return Ok(Target::Terminal(super::terminal::TerminalTarget {metadata,key:descriptor.key,generation:descriptor.generation,name:descriptor.name,
            created:descriptor.meta["created"].as_f64().unwrap_or(binding.created as f64),binding,
            plugin_key:descriptor.meta["plugin_key"].as_str().filter(|key|!key.is_empty()).map(str::to_owned),
            lease_path:descriptor.lock_path,state_path:descriptor.state_path,projection_dir:descriptor.projection_dir,transcript:descriptor.jsonl.into()}));
    }
    if descriptor.meta.get("terminal").is_some(){return Err(failure("descriptor_provider"));}
    let cano = &descriptor.meta["cano"];
    // Subida pedida sem cano gravado: pid 0 é "nenhum", e a subida grava o novo antes de conectar.
    let binding = if launch && cano["pid"].as_u64().is_none_or(|pid|pid == 0) { CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 } }
    else if launch { CanoBinding { pid:cano["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or_else(||failure("cano_pid"))?,
        escuta:cano["escuta"].as_str().unwrap_or_default().into(),token:cano["token"].as_str().unwrap_or_default().into(),
        // Cano vivo da sessão sem `versao` gravada já fala a 2.
        versao:cano["versao"].as_u64().and_then(|version|u32::try_from(version).ok()).unwrap_or(2) } }
    else { CanoBinding { pid:cano["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or_else(||failure("cano_pid"))?,
        escuta:cano["escuta"].as_str().ok_or_else(||failure("cano_address"))?.into(),
        token:cano["token"].as_str().ok_or_else(||failure("cano_token"))?.into(),
        // O Codex subido pelo Python grava o `cano` antes de conectar, sem `versao`, e já fala a 2.
        versao:match cano["versao"].as_u64().and_then(|version|u32::try_from(version).ok()) {
            Some(version)=>version, None if descriptor.provider == "codex"=>2, None=>return Err(failure("cano_version")) } } };
    Ok(Target::Headless(RuntimeTarget { key:descriptor.key,generation:descriptor.generation,name:descriptor.name,provider:descriptor.provider,
        created:descriptor.meta["created"].as_f64().unwrap_or(0.0),metadata:descriptor.meta,binding,
        lease_path:descriptor.lock_path,state_path:descriptor.state_path,projection_dir:descriptor.projection_dir,transcript:descriptor.jsonl.into() },
        descriptor.sidecar_dir))
}

async fn events(State(state):State<Gateway>) -> Response {
    let receiver = state.registry.subscribe();
    let initial = match state.registry.snapshots().await { Ok(events)=>std::collections::VecDeque::from(events),Err(error)=>{
        tracing::warn!(code=%error.code,reason=%error.message,"runtime sem retrato inicial dos eventos");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    } };
    let stream = futures_util::stream::unfold((receiver,initial),| (mut receiver,mut initial) | async move {
        let event = if let Some(event) = initial.pop_front() { event } else {
            match receiver.recv().await { Ok(event)=>event,Err(_)=>return None }
        };
        Some((Ok::<_,Infallible>(Event::default().event("runtime").data(serde_json::to_string(&event).unwrap())),(receiver,initial)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))).into_response()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::runtime::queue::acquire_lease;

    #[test]
    fn a_session_recreated_with_the_same_name_is_not_the_dead_life() {
        assert!(session_of_life("a\t10\njefferson-3\t500\n","jefferson-3",500));
        assert!(!session_of_life("a\t10\njefferson-3\t900\n","jefferson-3",500),"mesmo nome, outra criação: a vida morreu");
        assert!(!session_of_life("jefferson-30\t500\n","jefferson-3",500));
        assert!(session_of_life("jefferson-3\t\n","jefferson-3",500),"criação ilegível não prova outra vida");
    }

    /// Executor disputado, como no servidor carregado: cada volta da espera atrasa além dos 50 ms.
    #[tokio::test]
    async fn busy_lease_is_refused_after_three_seconds_of_clock_even_on_a_busy_executor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.lock");
        let _held = acquire_lease(&path).unwrap();
        let hog = tokio::spawn(async { loop { std::thread::sleep(Duration::from_millis(50)); tokio::task::yield_now().await; } });
        let started = std::time::Instant::now();
        let refused = wait_lease(&path).await;
        let elapsed = started.elapsed();
        hog.abort();
        assert_eq!(refused.unwrap_err().code,"runtime_lease");
        assert!(elapsed >= Duration::from_secs(3),"recusou antes dos 3 s: {elapsed:?}");
        assert!(elapsed < Duration::from_millis(4500),"a espera passou dos 3 s prometidos: {elapsed:?}");
    }

    struct SidRecipeProcesses {dir:std::path::PathBuf}
    impl crate::list::procs::ProcessView for SidRecipeProcesses {
        fn children(&self,_:Duration)->std::io::Result<Arc<crate::list::procs::ChildrenMap>> {Ok(Arc::new(std::collections::HashMap::from([(42,vec![43])]))) }
        fn argv(&self,_:i64)->Vec<String> {vec![]}
        fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(self.dir.clone())}
        fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
        fn start_time(&self,_:i64)->Option<f64> {Some(1000.0)}
        fn recipe(&self,pid:i64)->std::io::Result<crate::list::procs::ProcessRecipe> {
            let argv=match pid {42=>vec!["hangar-cano".into()],43=>vec!["claude".into(),"--session-id".into(),"a".into()],_=>return Err(std::io::ErrorKind::NotFound.into())};
            Ok(crate::list::procs::ProcessRecipe {pid,birth:1000.0,executable:if pid==42 {"/fixture/hangar-cano".into()} else {"/fixture/claude".into()},cwd:self.dir.clone(),argv,environ:vec![format!("CLAUDE_CONFIG_DIR={}",self.dir.display()).into()],cgroup:None})
        }
    }
    /// Cano Claude falso que responde o retrato e fica ouvindo.
    async fn fake_cano() -> (String,tokio::task::JoinHandle<()>) { fake_cano_gated(None).await }

    /// Com `gate`: avisa ao receber a conexão e só manda o retrato quando liberado.
    async fn fake_cano_gated(gate:Option<(Arc<tokio::sync::Notify>,Arc<tokio::sync::Notify>)>) -> (String,tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let mut line=String::new();socket.read_line(&mut line).await.unwrap();
            if let Some((accepted,release))=&gate {accepted.notify_one();release.notified().await;}
            let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            loop {line.clear();if socket.read_line(&mut line).await.unwrap_or(0)==0 {break;}}
        });
        (format!("tcp:{address}"),server)
    }

    /// Casa de teste com os sidecars dados em `.hangar/claude-headless/`; nada do HOME real é lido.
    fn claude_registry(home:&std::path::Path,sidecars:&[Value]) -> (Arc<RuntimeRegistry>,crate::side::Hubs) {
        let dir=home.join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        for meta in sidecars { std::fs::write(dir.join(format!("{}.json",meta["name"].as_str().unwrap())),meta.to_string()).unwrap(); }
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.to_string_lossy().into_owned())].into());
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into())
            .with_processes(Arc::new(SidRecipeProcesses {dir:home.into()})).with_codex_environment(env));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        (registry,hubs)
    }

    /// `/clear` sem terminal: a conversa nova do Claude passa à loja do plugin. Sem isso a loja ficava presa à
    /// antiga e todo fato lido dava `erro_interacao_desatualizada` ("estado talvez velho") até reabrir.
    #[tokio::test]
    async fn a_headless_clear_rebinds_the_plugin_store_to_the_new_conversation() {
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        let home=tempfile::tempdir().unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let mut line=String::new();socket.read_line(&mut line).await.unwrap();
            let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            // O `/clear` chega com a sessão já aberta.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let init=json!({"type":"cano_output","frame":json!({"type":"system","subtype":"init","session_id":"b"}).to_string()});
            socket.get_mut().write_all(format!("{init}\n").as_bytes()).await.unwrap();
            loop {line.clear();if socket.read_line(&mut line).await.unwrap_or(0)==0 {break;}}
        });
        let key="cd".repeat(16);
        let meta=json!({"name":"cl-rebind","key":key,"cwd":home.path(),"session_id":"a","headless":true,
            "permission_mode":"manual","cano":{"pid":42,"escuta":format!("tcp:{address}"),"token":"fixture","versao":2}}).to_string();
        // A troca de conversa grava o sidecar na pasta do ciclo de vida (a temporária, nos testes).
        for dir in [home.path().join(".hangar/claude-headless"),super::super::lifecycle::claude_sidecar_dir().unwrap()] {
            std::fs::create_dir_all(&dir).unwrap();std::fs::write(dir.join("cl-rebind.json"),&meta).unwrap();
        }
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned())].into());
        let plugin=Arc::new(crate::plugin::Store::new(Arc::new(crate::interactions::InteractionStore::default())));
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into())
            .with_processes(Arc::new(SidRecipeProcesses {dir:home.path().into()})).with_codex_environment(env).with_plugin(plugin.clone()));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        registry.open_claude_headless("cl-rebind",false).await.expect("é dela").expect("abriu");
        // Teto só contra travamento: no Windows do CI, com a bateria em paralelo, a troca passou de 5 s.
        let waited=tokio::time::timeout(Duration::from_secs(60),async {
            loop {
                if let Ok(binding)=registry.interaction_binding_readonly("cl-rebind").await && binding.conversation=="b" {break binding;}
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await;
        let Ok(binding)=waited else {
            let live=registry.live_sender("cl-rebind").borrow().as_ref().map(|s|(s.error.clone(),s.binding.as_ref().map(|b|(b.conversation.clone(),b.generation))));
            let entry=registry.entries.lock().await.get(&key).map(|e|(e.generation,e.name.clone(),e.respawn_inflight));
            panic!("a conversa nova não chegou à entrada: entrada {entry:?}, live {live:?}");
        };
        assert!(plugin.facts(&binding,std::time::Instant::now()).is_ok(),"a loja do plugin segue a conversa nova");
        registry.close(&key,binding.generation).await.unwrap();server.abort();
    }

    /// A escrita (ou a subida) acha a sessão parada pelo sidecar: o Rust a abre sozinho, administrada, sem
    /// o Python registrar antes; aberta, não abre de novo, e sem sidecar não é dela.
    #[tokio::test]
    async fn parked_claude_headless_opens_from_its_sidecar() {
        let home=tempfile::tempdir().unwrap();
        let (escuta,server)=fake_cano().await;
        let key="ab".repeat(16);
        let (registry,_hubs)=claude_registry(home.path(),&[json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,
            "permission_mode":"manual","cano":{"pid":42,"escuta":escuta,"token":"fixture","versao":2}})]);
        assert!(registry.open_claude_headless("outra",false).await.is_none(),"sem sidecar, não é sessão Claude sem terminal");
        registry.open_claude_headless("cl",false).await.expect("é dela").expect("abriu");
        assert!(registry.entries.lock().await.get(&key).unwrap().auto_respawn.is_some(),"o processo que cai é religado aqui");
        assert!(matches!(registry.open_claude_headless("cl",false).await,Some(Ok(()))),"aberta não abre de novo");
        registry.close(&key,1).await.unwrap();server.abort();
    }

    /// Entrada com o ator morto não conta como aberta: abrir fecha ela sem matar nada e sobe um ator de verdade,
    /// nunca responde `Ok` sem ator (a troca de conta no limite esperava a vista e caía em `relaunch_initialize`).
    #[tokio::test]
    async fn opening_over_a_dead_actor_really_opens() {
        let home=tempfile::tempdir().unwrap();
        let (escuta,server)=fake_cano().await;
        let key="ab".repeat(16);
        let meta=|escuta:&str|json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,
            "permission_mode":"manual","cano":{"pid":42,"escuta":escuta,"token":"fixture","versao":2}});
        let (registry,_hubs)=claude_registry(home.path(),&[meta(&escuta)]);
        registry.open_claude_headless("cl",false).await.expect("é dela").expect("abriu");
        let dead=registry.entries.lock().await.get(&key).unwrap().handle.clone();
        dead.stop().await.unwrap();
        // Ator que morreu antes de publicar a conversa: sem vínculo no canal.
        registry.live_sender("cl").send_replace(None);
        server.abort();
        let (escuta,server)=fake_cano().await;
        std::fs::write(home.path().join(".hangar/claude-headless/cl.json"),meta(&escuta).to_string()).unwrap();
        registry.open_claude_headless("cl",false).await.expect("é dela").expect("abriu");
        let handle=registry.entries.lock().await.get(&key).unwrap().handle.clone();
        assert!(handle.snapshot().await.is_ok(),"a entrada tem ator vivo");
        registry.close(&key,1).await.unwrap();server.abort();
    }

    /// Duas aberturas viram o mesmo ator morto: a que chega à barreira depois de a outra já ter reaberto
    /// (mesma geração, que vem do arquivo da fila) acha o ator vivo e não o fecha.
    #[tokio::test]
    async fn a_late_close_of_the_dead_actor_keeps_the_reopened_one() {
        let home=tempfile::tempdir().unwrap();
        let (escuta,server)=fake_cano().await;
        let key="ab".repeat(16);
        let meta=|escuta:&str|json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,
            "permission_mode":"manual","cano":{"pid":42,"escuta":escuta,"token":"fixture","versao":2}});
        let (registry,_hubs)=claude_registry(home.path(),&[meta(&escuta)]);
        registry.open_claude_headless("cl",false).await.expect("é dela").expect("abriu");
        registry.entries.lock().await.get(&key).unwrap().handle.clone().stop().await.unwrap();
        registry.live_sender("cl").send_replace(None);
        server.abort();
        let (escuta,server)=fake_cano().await;
        std::fs::write(home.path().join(".hangar/claude-headless/cl.json"),meta(&escuta).to_string()).unwrap();
        registry.open_claude_headless("cl",false).await.expect("é dela").expect("abriu");
        assert!(registry.close_if_dead(&key).await.unwrap(),"o ator reaberto está vivo");
        let handle=registry.entries.lock().await.get(&key).unwrap().handle.clone();
        assert!(handle.snapshot().await.is_ok(),"o fechamento atrasado não derrubou o ator reaberto");
        registry.close(&key,1).await.unwrap();server.abort();
    }

    /// Sidecar que existe e não se lê é erro visível, nunca "não é sessão Claude sem terminal".
    #[tokio::test]
    async fn an_unreadable_claude_sidecar_is_an_error_not_an_absent_session() {
        let home=tempfile::tempdir().unwrap();
        let (registry,_hubs)=claude_registry(home.path(),&[]);
        std::fs::write(home.path().join(".hangar/claude-headless/cl.json"),b"{torto").unwrap();
        let error=registry.open_claude_headless("cl",true).await.expect("o arquivo existe").expect_err("ilegível");
        assert_eq!(error.code,"claude_sidecar_read");
    }

    /// A 1ª mensagem chega enquanto a sessão recém-criada ainda sobe: a segunda abertura espera a primeira
    /// e acha a sessão aberta, sem disputar a trava da fila (que o ator segura a vida toda) nem marcar erro.
    #[tokio::test]
    async fn a_second_open_during_the_first_finds_the_session_open() {
        let home=tempfile::tempdir().unwrap();
        let (accepted,release)=(Arc::new(tokio::sync::Notify::new()),Arc::new(tokio::sync::Notify::new()));
        let (escuta,server)=fake_cano_gated(Some((accepted.clone(),release.clone()))).await;
        let key="ef".repeat(16);
        let (registry,_hubs)=claude_registry(home.path(),&[json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,
            "permission_mode":"manual","cano":{"pid":42,"escuta":escuta,"token":"fixture","versao":2}})]);
        let first={let registry=registry.clone();tokio::spawn(async move {registry.open_claude_headless("cl",false).await})};
        accepted.notified().await;
        let second={let registry=registry.clone();tokio::spawn(async move {registry.open_claude_headless("cl",false).await})};
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.notify_one();
        assert!(matches!(first.await.unwrap(),Some(Ok(()))));
        let second=tokio::time::timeout(Duration::from_secs(10),second).await.unwrap().unwrap();
        assert!(matches!(second,Some(Ok(()))),"a segunda abertura achou a sessão aberta: {:?}",second.map(|r|r.err().map(|e|e.code)));
        assert!(registry.runtime_lost("cl").await.is_none(),"sessão saudável sem erro marcado");
        registry.close(&key,1).await.unwrap();server.abort();
    }

    /// `_has_pending`: só a linha não entregue conta; fila ilegível também (abrir mostra a recusa dela).
    #[test]
    fn pending_counts_undelivered_rows_and_unreadable_queues() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("k.json");
        assert!(!pending(&path),"sem fila");
        std::fs::write(&path,json!({"rows":[{"id":"1","delivered":true}]}).to_string()).unwrap();
        assert!(!pending(&path));
        std::fs::write(&path,json!({"rows":[{"id":"1","delivered":true},{"id":"2","delivered":false}]}).to_string()).unwrap();
        assert!(pending(&path));
        std::fs::write(&path,"{").unwrap();
        assert!(pending(&path));
    }

    /// Na subida, a sessão parada sem nada a entregar fica fechada; a que tem mensagem na fila é subida
    /// (aqui falha: a casa de teste não tem `claude`, e a falha fica no canal dela).
    #[tokio::test]
    async fn boot_opens_only_live_or_pending_claude_headless_sessions() {
        let home=tempfile::tempdir().unwrap();
        let (parked,waiting)=("ab".repeat(16),"cd".repeat(16));
        let queue=home.path().join(".claude/.hangar-queue");std::fs::create_dir_all(queue.join("runtime")).unwrap();
        let row=json!({"id":"1","text":"oi","ts":1.0,"delivered":false});
        Store::open(&queue.join(format!("runtime/{waiting}.json")),&queue,QueueState::new(&waiting,1,"pendente",vec![row])).unwrap();
        let sidecar=|name:&str,key:&str|json!({"name":name,"key":key,"cwd":home.path(),"session_id":"a","headless":true,"permission_mode":"manual",
            "cano":{"pid":u32::MAX-7,"escuta":"tcp:127.0.0.1:1","token":"t","versao":2}});
        let (registry,_hubs)=claude_registry(home.path(),&[sidecar("parada",&parked),sidecar("pendente",&waiting)]);
        registry.clone().register_claude_headless().await;
        assert!(registry.runtime_lost("pendente").await.is_some(),"a pendente foi subida (e a falha aparece)");
        assert!(registry.runtime_lost("parada").await.is_none() && !registry.has_entry(None,"parada").await,"a parada fica fechada");
    }
    struct OwnSidecar(std::path::PathBuf);
    impl Drop for OwnSidecar {fn drop(&mut self) {let _=std::fs::remove_file(&self.0);}}
    /// Parity A3: Claude sem terminal aberto pelo Python com a pasta do sidecar fica administrado: o processo
    /// que cai é religado pelo Rust (`auto_respawn`), como o adapter antigo fazia.
    #[tokio::test]
    async fn claude_headless_opened_with_sidecar_dir_is_managed() {
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        let dir=tempfile::tempdir().unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let mut line=String::new();socket.read_line(&mut line).await.unwrap();
            let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            loop {line.clear();if socket.read_line(&mut line).await.unwrap_or(0)==0 {break;}}
        });
        let meta=json!({"key":"key","name":"cl","session_id":"a","headless":true,"initialized":true,"config_dir":dir.path(),"permission_mode":"manual",
            "cano":{"pid":42,"escuta":format!("tcp:{address}"),"token":"fixture","versao":2}});
        let descriptor=json!({"name":"cl","key":"key","provider":"claude","headless":true,"meta":meta,"jsonl":dir.path().join("a.jsonl"),
            "projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1,"sidecar_dir":dir.path()});
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into()).with_processes(Arc::new(SidRecipeProcesses {dir:dir.path().into()})));
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        let envelope=Envelope {protocol:0,instance:"instance".into(),key:"key".into(),generation:1,operation_id:"op".into(),clock:ClockSample {monotonic_s:0.0,epoch_s:0.0},
            command:json!({"kind":"open","descriptor":descriptor})};
        dispatch(&registry,&envelope).await.unwrap();
        assert!(registry.entries.lock().await.get("key").unwrap().auto_respawn.is_some(),"processo que cai fica sem dono");
        registry.close("key",1).await.unwrap();server.abort();
    }
    #[tokio::test]
    async fn opening_barrier_busy_sid_commit_retries_after_snapshot_and_release() {
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        use std::sync::atomic::AtomicUsize;
        let dir=tempfile::tempdir().unwrap();let name=format!("cx-sid-busy-{}",crate::mods::state::random_hex(8));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let users=Arc::new(AtomicUsize::new(0));let counted=users.clone();
        let server=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let mut line=String::new();socket.read_line(&mut line).await.unwrap();
            let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":json!({"type":"system","subtype":"init","session_id":"b"}).to_string(),"aberto":true,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            loop {line.clear();if socket.read_line(&mut line).await.unwrap_or(0)==0 {break;}
                let envelope:Value=serde_json::from_str(&line).unwrap();let frame:Value=serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();if frame["type"]=="user" {counted.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}
                let ack=json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});socket.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();}
        });
        let meta=json!({"key":"key","name":name,"session_id":"a","headless":true,"initialized":true,"config_dir":dir.path(),"permission_mode":"manual","previous_non_plan":"manual","cano":{"pid":42,"escuta":format!("tcp:{address}"),"token":"fixture","versao":2}});
        let sidecar_path=crate::runtime::lifecycle::claude_sidecar_dir().unwrap().join(format!("{name}.json"));std::fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();assert!(!sidecar_path.exists());std::fs::write(&sidecar_path,serde_json::to_vec(&meta).unwrap()).unwrap();let _sidecar=OwnSidecar(sidecar_path);
        let target=RuntimeTarget {key:"key".into(),generation:1,name:name.clone(),provider:"claude".into(),metadata:meta,binding:CanoBinding {pid:42,escuta:format!("tcp:{address}"),token:"fixture".into(),versao:2},lease_path:dir.path().join("lease"),state_path:dir.path().join("state"),projection_dir:dir.path().join("projection"),transcript:dir.path().join("a.jsonl"),created:0.0};std::fs::write(&target.transcript,b"").unwrap();
        let held_lease=acquire_lease(&target.lease_path).unwrap();let mut initial=QueueState::new("key",1,&name,vec![json!({"id":"a-row","text":"only-a","ts":1.0,"delivered":false})]);initial.runtime_state["view"]=json!({"conversation":"a","initialized":true});Store::open(&target.state_path,&target.projection_dir,initial).unwrap();drop(held_lease);
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into()).with_processes(Arc::new(SidRecipeProcesses {dir:dir.path().into()})).with_interactions(Arc::default()));let hubs=crate::side::Hubs::default();registry.bind_lifecycle();registry.bind_hubs(hubs.downgrade());
        let barrier=registry.barrier("key").await;let open_guard=barrier.lock().await;
        let opened=registry.open_headless_locked(target.clone(),None,false,true).await.unwrap();assert_eq!(opened["opened"],true);
        tokio::time::sleep(Duration::from_millis(100)).await;let pending=registry.handle("key",1).await.unwrap().snapshot().await.unwrap();assert!(pending["error"].is_null());assert_eq!(pending["view"]["lifecycle_pending"],true);assert_eq!(users.load(std::sync::atomic::Ordering::Relaxed),0);
        drop(open_guard);
        tokio::time::timeout(Duration::from_secs(2),async {loop {if registry.interaction_binding_readonly(&name).await.is_ok_and(|binding|binding.generation==2 && binding.conversation=="b") {break;}tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
        // Sem terminal, a medida segue o vínculo avançado (b, geração 2), não o metadado do boot (a).
        let (now,transcript)=registry.context_transcript(&name).await.unwrap();assert_eq!((now.generation,now.conversation.as_str()),(2,"b"));assert_eq!(transcript,dir.path().join("b.jsonl"));
        let measure=||crate::plugin::routes::MeasuredContext {used:7,window:200};
        let stale=crate::interactions::InteractionBinding {key:"key".into(),conversation:"a".into(),generation:1};
        assert_eq!(crate::plugin::sources::publish_context(&registry,&name,&stale,measure(),||panic!("vida antiga não invalida")).await.unwrap_err().0,"plugin_conversation_mismatch");
        crate::plugin::sources::publish_context(&registry,&name,&now,measure(),||{}).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("b.context.json")).unwrap(),r#"{"used":7,"window":200}"#);assert!(!dir.path().join("a.context.json").exists());
        let current=registry.handle("key",2).await.unwrap();let snapshot=current.snapshot().await.unwrap();assert!(snapshot["error"].is_null());assert_eq!(current.queue("load-b".into(),Action::Load).await.unwrap(),json!([]));assert_eq!(users.load(std::sync::atomic::Ordering::Relaxed),0);
        registry.close("key",2).await.unwrap();server.abort();
    }

    struct NoExternalIo;
    impl crate::terminal_input::TerminalIo for NoExternalIo {
        fn command<'a>(&'a self,_:crate::terminal_input::CommandRequest)->crate::terminal_input::IoFuture<'a,crate::terminal_input::CommandOutput> {
            Box::pin(async {Err(crate::terminal_input::IoFailure {code:"fixture_no_external_io",may_have_written:false})})
        }
        fn socket<'a>(&'a self,_:&'a crate::terminal_input::NativeMessage,_:Vec<u8>)->crate::terminal_input::IoFuture<'a,crate::terminal_input::WriteOutcome> {
            Box::pin(async {panic!("callback antigo não escreve no processo")})
        }
    }
    #[tokio::test]
    async fn open_of_adopted_terminal_accepts_the_python_birth_of_the_same_agent() {
        use crate::terminal_input::TerminalBinding;
        let dir=tempfile::tempdir().unwrap();
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        // Mesmo claude.exe medido na DELPHI-02: GetProcessTimes em f64 no Rust × psutil no Python.
        let target=|birth:f64|super::super::terminal::TerminalTarget {key:"K".into(),generation:1,name:"hangar".into(),
            binding:TerminalBinding {name:"hangar".into(),pane:"=hangar:0.0".into(),conversation:"sid".into(),generation:1,created:1,mux_argv:vec!["fixture-mux".into()],windows:true,clipboard_lock_path:None},
            metadata:crate::terminal_facts::TerminalMetadata {config_dir:dir.path().into(),agent_pid:9652,agent_birth:birth,legacy_import_after:None,fingerprint:"fixture".into(),origin:"uds:fixture".into()},
            state_path:dir.path().join("K.json"),lease_path:dir.path().join("K.lock"),projection_dir:dir.path().into(),transcript:dir.path().join("sid.jsonl"),created:1.0,plugin_key:None};
        let adopted=target(1792762169.8483868);
        let lease=acquire_lease(&adopted.lease_path).unwrap();
        let store=Store::open(&adopted.state_path,&adopted.projection_dir,QueueState::new("K",1,"hangar",vec![])).unwrap();
        let options=super::super::terminal::TerminalOptions {io:Arc::new(NoExternalIo),tick:Duration::from_secs(3600),..Default::default()};
        let handle=super::super::terminal::TerminalActor::spawn(adopted.clone(),QueueActor::start(store,lease),registry.policy.clone(),options,registry.events.clone(),Arc::new(AtomicU64::new(0)));
        registry.entries.lock().await.insert("K".into(),Entry {generation:1,handle:EntryHandle::Terminal {target:adopted.clone(),handle},lease_path:adopted.lease_path.clone(),name:"hangar".into(),mods_life:0,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding:crate::terminal_facts::LocalTerminalFacts::binding(&adopted),transcript:adopted.transcript.clone(),confirmed:None},live:registry.live_sender("hangar"),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        assert_eq!(registry.open_terminal(target(1792762169.8483875)).await.unwrap()["opened"],true);
        assert_eq!(registry.open_terminal(target(1792762170.8483868)).await.unwrap_err().code,"runtime_generation","outro processo no mesmo pid");
    }
    #[tokio::test]
    async fn dead_callback_a_keeps_b_actor_queue_and_interactions() {
        use crate::interactions::{InteractionBinding,InteractionStore};
        use crate::terminal_input::TerminalBinding;
        let dir=tempfile::tempdir().unwrap();
        let interactions=Arc::new(InteractionStore::default());
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()).with_interactions(interactions.clone());
        let binding=InteractionBinding {key:"B".into(),conversation:"new-sid".into(),generation:7};interactions.bind(&binding).unwrap();
        let target=super::super::terminal::TerminalTarget {key:"B".into(),generation:7,name:"same".into(),
            binding:TerminalBinding {name:"same".into(),pane:"%77".into(),conversation:"new-sid".into(),generation:7,created:1,mux_argv:vec!["fixture-mux".into()],windows:false,clipboard_lock_path:None},
            metadata:crate::terminal_facts::TerminalMetadata {config_dir:dir.path().into(),agent_pid:77,agent_birth:1000.0,legacy_import_after:None,fingerprint:"fixture-b".into(),origin:"uds:fixture".into()},
            state_path:dir.path().join("B.json"),lease_path:dir.path().join("B.lock"),projection_dir:dir.path().into(),transcript:dir.path().join("new-sid.jsonl"),created:1.0,plugin_key:None};
        std::fs::write(&target.transcript,b"").unwrap();
        let lease=acquire_lease(&target.lease_path).unwrap();
        let row=json!({"id":"row-b","text":"only-b","ts":1000.0,"delivered":false,"confirmed":false});
        let store=Store::open(&target.state_path,&target.projection_dir,QueueState::new("B",7,"same",vec![row.clone()])).unwrap();
        let options=super::super::terminal::TerminalOptions {io:Arc::new(NoExternalIo),tick:Duration::from_secs(3600),..Default::default()};
        let handle=super::super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),registry.policy.clone(),options,registry.events.clone(),Arc::new(AtomicU64::new(0)));
        registry.entries.lock().await.insert("B".into(),Entry {generation:7,handle:EntryHandle::Terminal {target:target.clone(),handle:handle.clone()},lease_path:target.lease_path.clone(),name:"same".into(),mods_life:0,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding:binding.clone(),transcript:target.transcript.clone(),confirmed:None},live:registry.live_sender("same"),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        assert_eq!(registry.dead("same","A","old-sid",1).await.unwrap(),crate::state::facts::Dead::InTransfer);
        assert!(registry.entries.lock().await.contains_key("B"));
        assert_eq!(handle.queue("fixture-load".into(),Action::Load).await.unwrap(),json!([row]));
        assert_eq!(registry.interaction_binding_readonly("same").await.unwrap(),binding);
        // Medida de contexto: a de A (SID e geração antigos) não toca B; falha de gravação aparece e não invalida a lista.
        let measure=||crate::plugin::routes::MeasuredContext {used:10,window:100};
        let invalidated=std::sync::atomic::AtomicUsize::new(0);let count=||{invalidated.fetch_add(1,std::sync::atomic::Ordering::Relaxed);};
        let old=InteractionBinding {key:"B".into(),conversation:"old-sid".into(),generation:1};
        assert_eq!(crate::plugin::sources::publish_context(&registry,"same",&old,measure(),count).await.unwrap_err().0,"plugin_conversation_mismatch");
        let blocked=dir.path().join("new-sid.context.json");std::fs::create_dir_all(blocked.join("keep")).unwrap();
        assert_eq!(crate::plugin::sources::publish_context(&registry,"same",&binding,measure(),count).await.unwrap_err().0,"plugin_context_write_failed");
        assert_eq!(invalidated.load(std::sync::atomic::Ordering::Relaxed),0);
        std::fs::remove_dir_all(&blocked).unwrap();
        crate::plugin::sources::publish_context(&registry,"same",&binding,measure(),count).await.unwrap();
        assert_eq!(std::fs::read_to_string(&blocked).unwrap(),r#"{"used":10,"window":100}"#);
        assert!(!dir.path().join("old-sid.context.json").exists());
        assert_eq!(invalidated.load(std::sync::atomic::Ordering::Relaxed),1);
        // Parity A5: o problema do ator chega à lista pelo último retrato, só o da vida aberta.
        let snapshot=|generation:u64,data:Value|RuntimeEvent {key:"B".into(),generation,revision:1,channel:"snapshot".into(),data};
        registry.note_terminal_problem(&snapshot(7,json!({"error":null,"view":{"terminal":true,"input_stalled":"composer_busy"}})));
        assert_eq!(registry.terminal_problems().await,BTreeMap::from([("same".to_owned(),"terminal_input_composer_busy".to_owned())]));
        registry.note_terminal_problem(&snapshot(7,json!({"error":"terminal_delivery_unknown","view":{"terminal":true}})));
        assert_eq!(registry.terminal_problems().await["same"],"runtime_falhou");
        registry.note_terminal_problem(&snapshot(7,json!({"error":"terminal_facts","view":{"terminal":true,"input_stalled":"no_safe_delivery"}})));
        assert_eq!(registry.terminal_problems().await["same"],"terminal_input_no_safe_delivery","a causa, não o erro que ela causa");
        registry.note_terminal_problem(&snapshot(6,json!({"error":"x","view":{"terminal":true}})));
        assert!(registry.terminal_problems().await.is_empty(),"retrato de outra vida não vale");
        registry.note_terminal_problem(&snapshot(7,json!({"error":null,"view":{"terminal":true}})));
        assert!(registry.terminal_problems().await.is_empty());
        handle.stop().await.unwrap();
    }

    /// Pane parado com o composer vazio; o pane e o agente são este processo. O multiplexador da
    /// reabertura (`ProcessIo` real com um programa que não existe) falha antes de qualquer efeito.
    struct IdlePane {working:Arc<std::sync::atomic::AtomicBool>}
    impl crate::terminal_input::TerminalIo for IdlePane {
        fn command<'a>(&'a self,r:crate::terminal_input::CommandRequest)->crate::terminal_input::IoFuture<'a,crate::terminal_input::CommandOutput> {
            Box::pin(async move {
                let spinner=if self.working.load(std::sync::atomic::Ordering::Acquire) {"✻ Sketching… (3s · esc to interrupt)\n"} else {""};
                let stdout=match r.args[0].as_str() {
                    "display-message" if r.args.last().is_some_and(|a|a.starts_with("#{pane_pid}"))=>format!("{}\theal\t%9\t1\n",std::process::id()).into_bytes(),
                    "display-message"=>b"heal\t%9\t1\n".to_vec(),
                    "capture-pane"=>format!("{spinner}────────────────────────────────\n❯ \n────────────────────────────────\n").into_bytes(),
                    _=>vec![],
                };
                Ok(crate::terminal_input::CommandOutput {success:true,stdout})
            })
        }
        fn socket<'a>(&'a self,_:&'a crate::terminal_input::NativeMessage,_:Vec<u8>)->crate::terminal_input::IoFuture<'a,crate::terminal_input::WriteOutcome> {
            Box::pin(async {panic!("a cura não escreve no socket")})
        }
    }
    /// Sem registro nativo e sem plugin, parada e com mensagem esperando: o ator pede a reabertura uma
    /// vez, e o registro recusa outra antes do intervalo, mesmo que a primeira tenha falhado.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn unproved_idle_session_with_queued_message_asks_one_heal() {
        use crate::interactions::InteractionBinding;
        use crate::terminal_input::TerminalBinding;
        let dir=tempfile::tempdir().unwrap();
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()));registry.bind_lifecycle();
        let pid=std::process::id();
        let target=super::super::terminal::TerminalTarget {key:"H".into(),generation:3,name:"heal".into(),
            binding:TerminalBinding {name:"heal".into(),pane:"%9".into(),conversation:"sid".into(),generation:3,created:1,mux_argv:vec!["hangar-fixture-no-such-mux".into()],windows:false,clipboard_lock_path:None},
            metadata:crate::terminal_facts::TerminalMetadata {config_dir:dir.path().into(),agent_pid:pid,agent_birth:crate::terminal_facts::agent_birth(pid).unwrap(),legacy_import_after:Some(1.0),fingerprint:"fixture-heal".into(),origin:"uds:fixture".into()},
            state_path:dir.path().join("H.json"),lease_path:dir.path().join("H.lock"),projection_dir:dir.path().into(),transcript:dir.path().join("projects/p/sid.jsonl"),created:1.0,plugin_key:None};
        std::fs::create_dir_all(target.transcript.parent().unwrap()).unwrap();std::fs::write(&target.transcript,b"").unwrap();
        let lease=acquire_lease(&target.lease_path).unwrap();
        let row=json!({"id":"row-h","text":"a","ts":1000.0,"delivered":false,"confirmed":false});
        let store=Store::open(&target.state_path,&target.projection_dir,QueueState::new("H",3,"heal",vec![row])).unwrap();
        let working=Arc::new(std::sync::atomic::AtomicBool::new(true));
        let options=super::super::terminal::TerminalOptions {io:Arc::new(IdlePane {working:working.clone()}),tick:Duration::from_millis(10),stall_notice:Duration::from_millis(20),
            lifecycle:Some(Arc::downgrade(&registry)),..Default::default()};
        let handle=super::super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),registry.policy.clone(),options,registry.events.clone(),Arc::new(AtomicU64::new(0)));
        let binding=InteractionBinding {key:"H".into(),conversation:"sid".into(),generation:3};
        registry.entries.lock().await.insert("H".into(),Entry {generation:3,handle:EntryHandle::Terminal {target:target.clone(),handle:handle.clone()},lease_path:target.lease_path.clone(),name:"heal".into(),mods_life:0,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding,transcript:target.transcript.clone(),confirmed:None},live:registry.live_sender("heal"),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        // Trabalhando: o aviso sai, a reabertura espera.
        tokio::time::timeout(Duration::from_secs(10),async {loop {
            if handle.snapshot().await.unwrap()["view"]["input_stalled"]=="no_safe_delivery" {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }}).await.expect("sem prova da conversa, o aviso aparece");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(registry.heal_attempts.lock().unwrap().is_empty(),"nunca com o Claude trabalhando");
        working.store(false,std::sync::atomic::Ordering::Release);
        tokio::time::timeout(Duration::from_secs(10),async {loop {
            if registry.heal_attempts.lock().unwrap().contains_key("H") {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }}).await.expect("a sessão parada e sem prova pede a reabertura");
        assert_eq!(handle.snapshot().await.unwrap()["view"]["input_stalled"],"no_safe_delivery");
        assert_eq!(registry.clone().heal_claude("heal".into(),"H".into(),3).await.unwrap_err().code,"heal_backoff");
        handle.stop().await.unwrap();
    }

    /// Sem entrada aqui, só o `claude` aberto à mão é tratado como "nunca foi do Rust": a abertura que falhou
    /// e a vida fechada com o processo vivo seguem com o código, sem parecer saudáveis.
    #[tokio::test]
    async fn failed_or_closed_rust_session_is_not_hand_opened() {
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        assert!(registry.hand_opened("solta").await);
        super::super::actor::mark_live_error(&registry.live_sender("falhou"),"cano_exited","o cano caiu ao abrir");
        assert!(!registry.hand_opened("falhou").await);
        assert_eq!(registry.runtime_lost("falhou").await.as_deref(),Some("cano_exited"));
        let dir=tempfile::tempdir().unwrap();
        let me=i64::from(std::process::id());
        let birth=crate::terminal_facts::agent_birth(std::process::id()).unwrap();
        let closed=super::super::lifecycle::ClosedLife {name:"fechada".into(),binding:crate::interactions::InteractionBinding {key:"K".into(),conversation:"c".into(),generation:1},
            pid:me,birth,state_path:dir.path().join("s"),lease_path:dir.path().join("l"),projection:dir.path().into()};
        *registry.barrier("K").await.closed.lock().unwrap()=Some(closed.clone());
        assert_eq!(registry.runtime_lost("fechada").await.as_deref(),Some("runtime_binding"),"processo ainda vivo");
        *registry.barrier("K").await.closed.lock().unwrap()=Some(super::super::lifecycle::ClosedLife {pid:i64::from(u32::MAX),..closed});
        assert!(registry.hand_opened("fechada").await,"vida antiga já acabou: o nome volta a ser de quem abrir");
    }

    /// Sessão Claude com terminal "same" registrada, sem tmux nem CLI de verdade.
    async fn claude_terminal_entry(registry:&RuntimeRegistry,dir:&std::path::Path,agent_pid:u32)->(super::super::terminal::TerminalTarget,super::super::terminal::TerminalHandle) {
        let target=super::super::terminal::TerminalTarget {key:"B".into(),generation:7,name:"same".into(),
            binding:crate::terminal_input::TerminalBinding {name:"same".into(),pane:"%77".into(),conversation:"new-sid".into(),generation:7,created:1,mux_argv:vec!["fixture-mux".into()],windows:false,clipboard_lock_path:None},
            metadata:crate::terminal_facts::TerminalMetadata {config_dir:dir.into(),agent_pid,agent_birth:1000.0,legacy_import_after:None,fingerprint:"fixture-b".into(),origin:"uds:fixture".into()},
            state_path:dir.join("B.json"),lease_path:dir.join("B.lock"),projection_dir:dir.into(),transcript:dir.join("new-sid.jsonl"),created:1.0,plugin_key:None};
        let lease=acquire_lease(&target.lease_path).unwrap();
        let store=Store::open(&target.state_path,&target.projection_dir,QueueState::new("B",7,"same",vec![])).unwrap();
        let options=super::super::terminal::TerminalOptions {io:Arc::new(NoExternalIo),tick:Duration::from_secs(3600),..Default::default()};
        let handle=super::super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),registry.policy.clone(),options,registry.events.clone(),Arc::new(AtomicU64::new(0)));
        registry.entries.lock().await.insert("B".into(),Entry {generation:7,handle:EntryHandle::Terminal {target:target.clone(),handle:handle.clone()},lease_path:target.lease_path.clone(),name:"same".into(),mods_life:0,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding:crate::terminal_facts::LocalTerminalFacts::binding(&target),transcript:target.transcript.clone(),confirmed:None},live:registry.live_sender("same"),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        (target,handle)
    }

    /// Parity M7: o Claude Code que não grava `sessions/<pid>.json` (ou o arquivo lido no meio da escrita)
    /// não deixa a sessão em erro a cada tique: sem sinal de troca, a conversa segue a mesma.
    #[tokio::test]
    async fn missing_native_record_is_no_conversation_change() {
        let dir=tempfile::tempdir().unwrap();
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let (_,handle)=claude_terminal_entry(&registry,dir.path(),77).await;
        assert_eq!(registry.reconcile_claude("same","B","new-sid",7).await.unwrap(),json!({"changed":false}));
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join("sessions/77.json"),br#"{"pid":77,"sessio"#).unwrap();
        assert_eq!(registry.reconcile_claude("same","B","new-sid",7).await.unwrap(),json!({"changed":false}),"lido no meio da escrita");
        handle.stop().await.unwrap();
    }

    /// Parity M5: a primeira leitura já em `plan` não inventa `manual` como base dos recados; sem a base
    /// lida, decide o transcript ou o modo de abertura (`session_non_plan_mode`).
    #[tokio::test]
    async fn guessed_plan_base_does_not_decide_the_message_class() {
        let dir=tempfile::tempdir().unwrap();
        let memory=Arc::new(std::sync::Mutex::new(crate::state::permission::PermissionMemory::default()));
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()).with_permissions(memory.clone());
        let (_,handle)=claude_terminal_entry(&registry,dir.path(),77).await;
        let read=registry.terminal_mode_reader();
        memory.lock().unwrap().observe("new-sid","plan");
        assert_eq!(read("same".into()).await.unwrap().unwrap().confirmed,None,"base suposta não vira `manual`");
        memory.lock().unwrap().observe("new-sid","bypassPermissions");
        memory.lock().unwrap().observe("new-sid","plan");
        assert_eq!(read("same".into()).await.unwrap().unwrap().confirmed.as_deref(),Some("bypassPermissions"));
        // Decisão M6: a classe do recado sai daqui para o Python; remetente desconhecido cai no alvo.
        assert_eq!(registry.message_class(Some("ninguem"),"same").await.unwrap(),"bypass");
        // Alvo sem entrada é erro com código (o Python não recalcula com o Rust de pé).
        assert_eq!(registry.message_class(None,"fora").await.unwrap_err().code,"runtime_binding");
        // Sem modo confirmado, o Rust mesmo cai no transcript e na conta, como o Python fazia.
        *memory.lock().unwrap()=crate::state::permission::PermissionMemory::default();
        std::fs::write(dir.path().join("settings.json"),r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
        assert_eq!(registry.message_class(None,"same").await.unwrap(),"bypass");
        handle.stop().await.unwrap();
    }

    /// Aviso do painel para sessão recém-aberta: sem rodapé lido nem modo no transcript, vale o modo com que
    /// o CLI nasceu (argv), não o `defaultMode` da conta, que faria o Claude reter o recado.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn panel_notice_to_a_fresh_session_uses_the_launch_mode_before_the_account() {
        let dir=tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"),r#"{"permissions":{"defaultMode":"auto"}}"#).unwrap();
        let mut agent=std::process::Command::new("sh").args(["-c","sleep 30; :","claude","--permission-mode","bypassPermissions"]).spawn().unwrap();
        // Antes do exec o filho ainda tem o argv do teste.
        const FLAG:&[u8]=b"bypassPermissions";
        let deadline=std::time::Instant::now()+Duration::from_secs(10);
        while !std::fs::read(format!("/proc/{}/cmdline",agent.id())).unwrap_or_default().windows(FLAG.len()).any(|w|w==FLAG) {
            assert!(std::time::Instant::now()<deadline,"o filho não chegou ao exec");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let memory=Arc::new(std::sync::Mutex::new(crate::state::permission::PermissionMemory::default()));
        let registry=RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()).with_permissions(memory);
        let (target,handle)=claude_terminal_entry(&registry,dir.path(),agent.id()).await;
        let class=registry.message_class(None,"same").await;
        // O transcript, quando já traz o modo, é mais novo que o argv e vence.
        std::fs::write(&target.transcript,"{\"permissionMode\":\"acceptEdits\"}\n").unwrap();
        let from_transcript=registry.message_class(None,"same").await;
        agent.kill().unwrap();agent.wait().unwrap();
        handle.stop().await.unwrap();
        assert_eq!(class.unwrap(),"bypass");
        assert_eq!(from_transcript.unwrap(),"prompting");
    }

    /// No encerramento do servidor os hubs caem antes do registro: `shutdown` fecha TODAS as sessões
    /// Claude mesmo assim. Hubs nunca ligados continuam erro.
    #[tokio::test]
    async fn shutdown_after_hubs_dropped_closes_every_claude_entry() {
        use crate::interactions::InteractionBinding;
        use crate::terminal_input::TerminalBinding;
        let dir=tempfile::tempdir().unwrap();
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()));
        for (key,name) in [("A","a"),("B","b")] {
            let binding=InteractionBinding {key:key.into(),conversation:format!("{name}-sid"),generation:1};
            let target=super::super::terminal::TerminalTarget {key:key.into(),generation:1,name:name.into(),
                binding:TerminalBinding {name:name.into(),pane:"%1".into(),conversation:binding.conversation.clone(),generation:1,created:1,mux_argv:vec!["fixture-mux".into()],windows:false,clipboard_lock_path:None},
                metadata:crate::terminal_facts::TerminalMetadata {config_dir:dir.path().into(),agent_pid:77,agent_birth:1000.0,legacy_import_after:None,fingerprint:format!("fixture-{name}"),origin:"uds:fixture".into()},
                state_path:dir.path().join(format!("{key}.json")),lease_path:dir.path().join(format!("{key}.lock")),projection_dir:dir.path().into(),transcript:dir.path().join(format!("{name}.jsonl")),created:1.0,plugin_key:None};
            std::fs::write(&target.transcript,b"").unwrap();
            let lease=acquire_lease(&target.lease_path).unwrap();
            let store=Store::open(&target.state_path,&target.projection_dir,QueueState::new(key,1,name,vec![])).unwrap();
            let options=super::super::terminal::TerminalOptions {io:Arc::new(NoExternalIo),tick:Duration::from_secs(3600),..Default::default()};
            let handle=super::super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),registry.policy.clone(),options,registry.events.clone(),Arc::new(AtomicU64::new(0)));
            registry.entries.lock().await.insert(key.into(),Entry {generation:1,handle:EntryHandle::Terminal {target:target.clone(),handle},lease_path:target.lease_path.clone(),name:name.into(),mods_life:0,provider:"claude".into(),account_key:None,relaunch:None,settings_owner:None,
                mode_source:crate::terminal_facts::ModeSource {binding,transcript:target.transcript.clone(),confirmed:None},live:registry.live_sender(name),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        }
        let never=crate::interactions::InteractionBinding {key:"X".into(),conversation:"x".into(),generation:1};
        assert_eq!(registry.forget_claude_rate(&never).unwrap_err().code,"lifecycle_hubs_unbound");
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());drop(hubs);
        registry.shutdown().await.unwrap();
        assert!(registry.entries.lock().await.is_empty(),"nenhuma sessão ficou sem fechar");
    }

    #[tokio::test]
    async fn terminal_status_is_independent_from_global_events_and_cleans_healthy_names() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into());
        let sender = registry.terminal_status_sender("a");
        let _other = registry.terminal_status("b");
        let mut receiver = registry.terminal_status("a");
        let mut global = registry.subscribe();
        assert_eq!(registry.events.receiver_count(), 1, "o canal por sessão não assina o global");
        for i in 0..10 {
            assert!(registry.events.send(RuntimeEvent { key: format!("other-{i}"), generation:1, revision:1, channel:"snapshot".into(), data:json!({"view":{"text":"large unrelated state"}}) }).is_ok());
        }
        assert!(!receiver.has_changed().unwrap());
        assert_eq!(global.recv().await.unwrap().key, "other-0", "o consumidor Python continua recebendo os eventos");
        sender.send_replace(super::super::terminal_status::TerminalStatus { input_stalled:Some("composer_busy".into()), ..Default::default() });
        assert_eq!(receiver.borrow_and_update().input_stalled.as_deref(), Some("composer_busy"));
        drop(sender);
        registry.clear_live("a").await;
        assert_eq!(*receiver.borrow(), Default::default());
        drop(receiver);
        let _new = registry.terminal_status("c");
        assert!(!registry.terminal_status.lock().unwrap().contains_key("a"));
    }

    #[tokio::test]
    async fn terminal_status_failed_close_keeps_error_detail_for_reconnection() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into());
        let sender = registry.terminal_status_sender("a");
        super::super::terminal_status::publish_problem(&sender, "queue_io", "fila indisponível");
        registry.clear_live("a").await;
        let receiver = registry.terminal_status("a");
        assert_eq!(receiver.borrow().error.as_deref(), Some("queue_io"));
        assert_eq!(receiver.borrow().message.as_deref(), Some("fila indisponível"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_status_concurrent_updates_stay_with_their_ten_sessions() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into());
        let barrier = Arc::new(tokio::sync::Barrier::new(10));
        let mut receivers = Vec::new();
        let mut publishers = tokio::task::JoinSet::new();
        for i in 0..10 {
            let name = format!("session-{i}");
            let sender = registry.terminal_status_sender(&name);
            receivers.push((name.clone(), registry.terminal_status(&name)));
            let barrier = barrier.clone();
            publishers.spawn(async move {
                barrier.wait().await;
                sender.send_replace(super::super::terminal_status::TerminalStatus { input_stalled:Some(name), ..Default::default() });
            });
        }
        while let Some(result) = publishers.join_next().await { result.unwrap(); }
        for (name, mut receiver) in receivers {
            tokio::time::timeout(Duration::from_secs(1), receiver.changed()).await.unwrap().unwrap();
            assert_eq!(receiver.borrow().input_stalled.as_deref(), Some(name.as_str()));
        }
        assert_eq!(registry.events.receiver_count(), 0);
    }

    #[test]
    fn healthy_follows_python_rust_failed() {
        assert!(!healthy(true,&json!({"error":"terminal_facts","view":{}})));
        assert!(healthy(true,&json!({"error":"receipt_scan","view":{}})));
        assert!(!healthy(true,&json!({"error":"queue_io","view":{}})));
        assert!(healthy(true,&json!({"error":null,"view":{}})));
        assert!(!healthy(false,&json!({"error":null,"view":{"alive":false}})));
        assert!(!healthy(false,&json!({"error":"cano_exited","view":{"alive":true}})));
        assert!(healthy(false,&json!({"error":null,"view":{"alive":true}})));
        assert!(healthy(false,&json!({"error":"","view":{}})));
    }

    #[test]
    fn terminal_and_headless_sources_do_not_overlap() {
        let value = json!({"name":"session","key":"key","provider":"codex","headless":false,
            "meta":{"key":"key","cano":{"pid":42,"escuta":"tcp:127.0.0.1:1","token":"test","versao":2}},
            "jsonl":"chat.jsonl","projection_dir":"projection","state_path":"state","lock_path":"lock","generation":1});
        assert!(descriptor(&value,false).is_err());
        let mut headless = value;
        headless["headless"] = json!(true);
        assert!(descriptor(&headless,false).is_ok());
    }

    #[test]
    fn codex_terminal_never_opens_over_the_gateway() {
        // Quem adota o Codex com terminal é a vigia do Rust: o descritor do Python é recusado, com ou sem `cano`.
        let value = json!({"name":"cx-proof","key":"codex-key","provider":"codex","headless":false,
            "meta":{"key":"codex-key","thread_id":"thread-1","endpoint":"ws://127.0.0.1:43210","app_pid":12345,"cano":null,
                "terminal":{"name":"cx-proof","pane":"%7","conversation":"thread-1","generation":1,"created":1,"mux_argv":["tmux"],"windows":false,"clipboard_lock_path":null}},
            "jsonl":"","projection_dir":"projection","state_path":"state","lock_path":"lock","generation":1});
        for launch in [false,true] { assert_eq!(descriptor(&value,launch).err().map(|e|e.code).as_deref(),Some("descriptor_provider")); }
        let mut without = value;
        without["meta"].as_object_mut().unwrap().remove("cano");
        assert_eq!(descriptor(&without,false).err().map(|e|e.code).as_deref(),Some("descriptor_provider"));
    }

    #[test]
    fn closing_an_old_life_does_not_unmark_the_live_owner_of_the_same_name() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        registry.mark_codex_owner("cx","old-key",1,true);
        registry.mark_codex_owner("cx","new-key",1,true);
        registry.mark_codex_owner("cx","old-key",1,false);
        assert!(registry.owns_codex_terminal("cx"),"a vida nova segue dona do estado");
        registry.mark_codex_owner("cx","new-key",2,false);
        assert!(registry.owns_codex_terminal("cx"),"outra geração da mesma chave também não desmarca");
        registry.mark_codex_owner("cx","new-key",1,false);
        assert!(!registry.owns_codex_terminal("cx"));
    }

    #[test]
    fn launch_accepts_a_session_without_cano() {
        let value = json!({"name":"session","key":"key","provider":"codex","headless":true,"meta":{"key":"key","cano":null},
            "jsonl":"","projection_dir":"projection","state_path":"state","lock_path":"lock","generation":1,"sidecar_dir":"dir"});
        assert!(descriptor(&value,false).is_err(),"sem `launch`, sem cano não abre");
        let Ok(Target::Headless(target,sidecar_dir)) = descriptor(&value,true) else { panic!("subida sem cano gravado") };
        assert_eq!((target.binding.pid,target.binding.versao),(0,2));
        assert_eq!(sidecar_dir.as_deref(),Some(std::path::Path::new("dir")));
    }

    /// Nada da subida vai ao Python (porta fechada): o Codex grava o arquivo da sessão e monta o comando;
    /// o Claude grava o arquivo da sessão e nunca sobe por aqui.
    #[test]
    fn a_cano_the_session_file_did_not_take_is_a_failure() {
        assert!(cano_recorded(Ok(json!({"updated":true}))).is_ok());
        assert_eq!(cano_recorded(Ok(json!({"updated":false,"stale":true}))).unwrap_err().code,"session_patch_stale");
        assert_eq!(cano_recorded(Err(RuntimeError::new("claude_sidecar_io","x"))).unwrap_err().code,"claude_sidecar_io");
    }

    #[tokio::test]
    async fn sidecar_launch_policies_never_reach_python() {
        let dir = tempfile::tempdir().unwrap();
        let policy = PolicyClient::new("127.0.0.1:9".parse().unwrap(),"test".into())
            .with_codex_environment(crate::accounts::environment::AccountEnvironment::from_map(
                [("HOME".to_owned(),dir.path().to_string_lossy().into_owned()),("PATH".to_owned(),dir.path().join("bin").to_string_lossy().into_owned())].into()));
        std::fs::write(super::super::codex_sidecar::path(dir.path(),"cx"),json!({"name":"cx","key":"k","headless":true,"cano":{"pid":7}}).to_string()).unwrap();
        std::fs::write(dir.path().join("q.json"),json!({"runtime_state":{"view":{"model":"m"}}}).to_string()).unwrap();
        let mut target = RuntimeTarget { key:"k".into(),generation:1,name:"cx".into(),provider:"codex".into(),
            metadata:json!({"name":"cx","key":"k","headless":true,"cwd":dir.path()}),
            binding:CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 },
            lease_path:dir.path().join("q.lock"),state_path:dir.path().join("q.json"),projection_dir:dir.path().join("projection"),
            transcript:dir.path().join("rollout.jsonl"),created:0.0 };
        assert_eq!(launch_sidecar(&target,dir.path(),"session.patch_meta",json!({"model":"m"})).await.unwrap(),json!({"updated":true}));
        assert_eq!(launch_sidecar(&target,dir.path(),"session.patch_meta",json!({"model":"x"})).await.unwrap()["stale"],true,"a vista do arquivo da fila vale");
        assert_eq!(launch_sidecar(&target,dir.path(),"session.clear_cano",json!({"pid":7})).await.unwrap(),json!({"cleared":true}));
        // Sem `codex` no PATH injetado a subida falha no processo, nunca numa política do Python.
        let failed = launch_if_needed(&policy,&mut target,dir.path()).await.err().expect("a subida sem codex precisa falhar");
        assert!(!failed.code.starts_with("policy_"),"{}",failed.code);
        let claude = RuntimeTarget { provider:"claude".into(),name:"cl".into(),..target };
        std::fs::write(super::super::codex_sidecar::path(dir.path(),"cl"),json!({"name":"cl","key":"k","headless":true,"cano":{"pid":7}}).to_string()).unwrap();
        assert_eq!(launch_sidecar(&claude,dir.path(),"session.patch_meta",json!({"cano":{"pid":8}})).await.unwrap(),json!({"updated":true}));
        assert_eq!(launch_sidecar(&claude,dir.path(),"session.patch_meta",json!({"model":"x"})).await.unwrap()["stale"],true,"a vista do arquivo da fila vale");
        assert_eq!(launch_sidecar(&claude,dir.path(),"session.clear_cano",json!({"pid":8})).await.unwrap(),json!({"cleared":true}));
        // O Claude sobe pelo spec da sessão: sem o cano no PATH injetado, falha no processo.
        let mut claude = RuntimeTarget { metadata:json!({"name":"cl","key":"k","headless":true,"cwd":dir.path(),
            "session_id":"0c657c79-1745-406b-a97f-bbbf5402ff19","permission_mode":"manual"}),..claude };
        let failed = launch_if_needed(&policy,&mut claude,dir.path()).await.err().expect("a subida sem cano precisa falhar");
        assert!(!failed.code.starts_with("policy_") && failed.code != "relaunch_recipe_missing","{}",failed.code);
        // Pid gravado vivo mas de outro programa (reaproveitado depois do boot) não é o cano da
        // sessão: sobe outro em vez de dar a sessão por de pé.
        claude.binding.pid = std::process::id();
        assert!(launch_if_needed(&policy,&mut claude,dir.path()).await.is_err(),"pid alheio não vale como cano vivo");
        // Sem terminal não há bwrap: só leitura nunca sobe aqui.
        claude.metadata["read_only"] = json!(true);
        assert_eq!(launch_if_needed(&policy,&mut claude,dir.path()).await.err().unwrap().code,"headless_read_only");
    }

    #[test]
    fn held_empty_live_channel_survives_another_lookup() {
        // O `open` pega o canal e o ator só escreve no primeiro passo: outra procura no meio não o apaga.
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let held = registry.live_sender("a");
        let _other = registry.live_sender("b");
        held.send_replace(Some(Arc::new(LiveState::default())));
        assert!(registry.live("a").borrow().is_some(),"o feed lê o mesmo canal que o ator segura");
        drop(held);
        registry.live_sender("a").send_replace(None);
        let _ = registry.live_sender("c");
        assert!(!registry.live.lock().unwrap().contains_key("a"),"vazio, sem dono e sem receptor: sai");
    }

    #[tokio::test]
    async fn failed_codex_open_shows_its_error_until_the_next_open() {
        let dir = tempfile::tempdir().unwrap();
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let rx = registry.live("cx");
        // Cano gravado que não atende: a conexão falha.
        let target = RuntimeTarget { key:"k".into(),generation:1,name:"cx".into(),provider:"codex".into(),
            metadata:json!({"name":"cx","key":"k","headless":true}),
            binding:CanoBinding { pid:42,escuta:"tcp:127.0.0.1:9".into(),token:"t".into(),versao:2 },
            lease_path:dir.path().join("q.lock"),state_path:dir.path().join("q.json"),projection_dir:dir.path().join("projection"),
            transcript:dir.path().join("rollout.jsonl"),created:0.0 };
        let error = registry.open(target.clone()).await.unwrap_err();
        assert_eq!(rx.borrow().as_ref().and_then(|s|s.error.clone()).map(|(code,_)|code),Some(error.code.clone()));
        // Pedido torto (provedor de fora) não acende problema.
        registry.live_sender("cx").send_replace(None);
        let other = RuntimeTarget { provider:"pi".into(),..target };
        assert_eq!(registry.open(other).await.unwrap_err().code,"runtime_provider");
        assert!(rx.borrow().is_none());
    }

    #[tokio::test]
    async fn close_keeps_the_error_of_a_life_that_failed() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let rx = registry.live("a");
        let failed = LiveState { error:Some(("queue_io".into(),"fila recusou".into())),..Default::default() };
        registry.live_sender("a").send_replace(Some(Arc::new(failed)));
        registry.clear_live("a").await;
        assert!(rx.borrow().as_ref().is_some_and(|s|s.error.is_some()),"o hub segue mostrando a falha depois do close");
        registry.live_sender("a").send_replace(Some(Arc::new(LiveState::default())));
        registry.clear_live("a").await;
        assert!(rx.borrow().is_none(),"vida que acabou bem: o canal esvazia");
    }

    #[tokio::test]
    async fn close_leaves_the_channels_of_a_new_life_with_the_same_name() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        // A vida nova já pegou os canais e escreveu, mas ainda não entrou em `entries`.
        let status = registry.terminal_status_sender("a");
        status.send_replace(super::super::terminal_status::TerminalStatus { input_stalled:Some("delivery_unproved".into()),..Default::default() });
        let live = registry.live_sender("a");
        live.send_replace(Some(Arc::new(LiveState::default())));
        registry.clear_live("a").await;
        assert_eq!(registry.terminal_status("a").borrow().input_stalled.as_deref(),Some("delivery_unproved"));
        assert!(registry.live("a").borrow().is_some(),"o canal da vida nova segue com o valor dela");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribing_readers_do_not_prevent_closed_channels_from_clearing() {
        let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()));
        let status = registry.terminal_status("a");
        let live = registry.live("a");
        let reader_registry = registry.clone();
        let readers = std::thread::spawn(move || {
            for _ in 0..10_000 {
                let _status = reader_registry.terminal_status("a");
                let _live = reader_registry.live("a");
            }
        });
        for _ in 0..1_000 {
            registry.terminal_status_sender("a").send_replace(super::super::terminal_status::TerminalStatus {
                input_stalled:Some("composer_busy".into()),..Default::default()
            });
            registry.live_sender("a").send_replace(Some(Arc::new(LiveState::default())));
            registry.clear_live("a").await;
            assert_eq!(*status.borrow(), Default::default(),"assinar não conserva o bloqueio encerrado");
            assert!(live.borrow().is_none(),"assinar não conserva a vida encerrada");
        }
        readers.join().unwrap();
    }

    #[tokio::test]
    async fn one_sse_reader_multiple_keys() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let mut receiver = registry.subscribe();
        assert_eq!(registry.events.receiver_count(),1);
        for key in ["claude-key","codex-key"] {
            assert!(registry.events.send(RuntimeEvent { key:key.into(),generation:1,revision:1,channel:"state".into(),data:json!({}) }).is_ok());
        }
        assert_eq!(receiver.recv().await.unwrap().key,"claude-key");
        assert_eq!(receiver.recv().await.unwrap().key,"codex-key");
    }

    /// Cano de verdade (um `sh` com o argv do cano da chave, em grupo próprio) e um `claude` de mentira filho
    /// dele, que some junto com o cano: o bastante para o fechamento provar e matar a vida.
    #[cfg(target_os="linux")]
    /// `agent`: um processo de verdade fora do grupo do cano faz o `claude`, e sobrevive a ele.
    struct ParkProcesses {canos:std::sync::Mutex<Vec<i64>>,dir:std::path::PathBuf,agent:std::sync::Mutex<Option<i64>>}
    #[cfg(target_os="linux")]
    const FAKE_AGENT:i64=10_000_000;
    #[cfg(target_os="linux")]
    impl crate::list::procs::ProcessView for ParkProcesses {
        fn children(&self,_:Duration)->std::io::Result<Arc<crate::list::procs::ChildrenMap>> {
            let agent=*self.agent.lock().unwrap();
            Ok(Arc::new(self.canos.lock().unwrap().iter().filter(|pid|process::probe_alive(**pid as u32)).map(|pid|(*pid,vec![agent.unwrap_or(pid+FAKE_AGENT)])).collect()))
        }
        fn argv(&self,_:i64)->Vec<String> {vec![]}
        fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(self.dir.clone())}
        fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
        fn start_time(&self,pid:i64)->Option<f64> {crate::list::procs::SystemProcs::default().start_time(pid)}
        fn recipe(&self,pid:i64)->std::io::Result<crate::list::procs::ProcessRecipe> {
            let environ=vec![format!("CLAUDE_CONFIG_DIR={}",self.dir.display()).into()];
            if *self.agent.lock().unwrap()==Some(pid) {
                let birth=crate::list::procs::SystemProcs::default().recipe(pid)?.birth;
                return Ok(crate::list::procs::ProcessRecipe {pid,birth,executable:"/fixture/claude".into(),cwd:self.dir.clone(),argv:vec!["claude".into(),"--session-id".into(),"a".into()],environ,cgroup:None});
            }
            let canos=self.canos.lock().unwrap().clone();
            let cano=if canos.contains(&pid) {pid} else {pid-FAKE_AGENT};
            if !canos.contains(&cano) || !process::probe_alive(cano as u32) {return Err(std::io::ErrorKind::NotFound.into());}
            let (birth,argv,executable)=if pid==cano {(crate::terminal_facts::agent_birth(cano as u32).unwrap(),vec!["hangar-cano".into()],"/fixture/hangar-cano")}
                else {(1000.0,vec!["claude".into(),"--session-id".into(),"a".into()],"/fixture/claude")};
            Ok(crate::list::procs::ProcessRecipe {pid,birth,executable:executable.into(),cwd:self.dir.clone(),argv,environ:vec![format!("CLAUDE_CONFIG_DIR={}",self.dir.display()).into()],cgroup:None})
        }
    }
    /// Processo do teste que nunca sobra: morto e colhido na limpeza, também quando o teste falha.
    #[cfg(target_os="linux")]
    struct Reaped(std::process::Child);
    #[cfg(target_os="linux")]
    impl Drop for Reaped {fn drop(&mut self) {let _=self.0.kill();let _=self.0.wait();}}
    /// O processo que se passa pelo cano da chave e o cano falso que responde por ele e anota cada linha recebida.
    #[cfg(target_os="linux")]
    async fn park_cano(dir:&std::path::Path,key:&str) -> (Reaped,String,Arc<std::sync::Mutex<Vec<String>>>,tokio::task::JoinHandle<()>) {
        use std::os::unix::process::CommandExt;
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        let log=dir.join(format!("cano-{}.log",&key[..16]));
        let child=Reaped(std::process::Command::new("python3").args(["-c","import time; time.sleep(300)","--escuta","fixture","--log"]).arg(&log).process_group(0).spawn().unwrap());
        let deadline=std::time::Instant::now()+Duration::from_secs(10);
        while process::liveness(child.0.id(),key)!=process::Liveness::Ours {
            assert!(std::time::Instant::now()<deadline,"o processo do cano não chegou ao argv");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let lines=Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen=lines.clone();
        let server=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let mut line=String::new();socket.read_line(&mut line).await.unwrap();
            let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            loop {line.clear();if socket.read_line(&mut line).await.unwrap_or(0)==0 {break;} seen.lock().unwrap().push(line.clone());}
        });
        (child,format!("tcp:{address}"),lines,server)
    }

    /// O vigia estaciona só a sessão parada há o prazo e sem nada na fila; o processo morre e o arquivo
    /// da sessão fica igual. A mensagem seguinte reabre a sessão pelo arquivo e é entregue uma vez.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn idle_claude_headless_is_parked_and_the_next_message_reopens_it() {
        let home=tempfile::tempdir().unwrap();
        let key="ab".repeat(16);
        let (mut first,escuta,_,server)=park_cano(home.path(),&key).await;
        let sidecar=json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":first.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        let dir=home.path().join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cl.json"),sidecar.to_string()).unwrap();
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(first.0.id())]),dir:home.path().into(),agent:Default::default()});
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned())].into());
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into()).with_processes(processes.clone()).with_codex_environment(env));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        registry.open_claude_headless("cl",false).await.unwrap().unwrap();
        let state_path=registry.claude_dirs().1.join("runtime").join(format!("{key}.json"));

        registry.park_idle_claude(Duration::from_secs(65*60)).await;
        assert!(registry.has_entry(Some(&key),"cl").await && process::probe_alive(first.0.id()),"parada há menos que o prazo fica aberta");

        // A fila é do ator, que a regrava da memória: a mensagem entra e sai por ele. O aviso de limite a
        // segura sem entregar, e o estacionamento não olha esse aviso, só a fila.
        let (sink,_hits)=tokio::sync::mpsc::unbounded_channel();
        registry.set_limit_sink(sink);
        registry.usage_limit(LimitHit {name:"cl".into(),provider:"claude".into(),account:None,limit:UsageLimit {window:"five_hour".into(),resets_at:None}});
        registry.queue_action("cl","x".into(),Action::Append {text:"t".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("x".into())}).await.unwrap();
        registry.park_idle_claude(Duration::ZERO).await;
        assert!(registry.has_entry(Some(&key),"cl").await && process::probe_alive(first.0.id()),"mensagem por entregar na fila segura a sessão");
        assert_eq!(registry.queue_action("cl","x-cancel".into(),Action::Cancel {entry_id:"x".into()}).await.unwrap(),json!("cancelled"));
        registry.release_limit("cl");

        let before=std::fs::read(dir.join("cl.json")).unwrap();
        registry.park_idle_claude(Duration::ZERO).await;
        assert!(!registry.has_entry(None,"cl").await,"estacionada sai do Rust");
        first.0.wait().unwrap();
        assert!(!process::probe_alive(first.0.id()),"o processo da sessão morreu");
        assert_eq!(std::fs::read(dir.join("cl.json")).unwrap(),before,"o arquivo da sessão fica igual");
        assert!(state_path.exists(),"a fila fica");
        assert!(registry.runtime_lost("cl").await.is_none(),"estacionada não é sessão perdida");
        assert_eq!(crate::state::parked::parked_state_at("cl",home.path()).map(|state|state.state),Some("idle".into()),"a lista mostra a parada");
        let diary=home.path().join(".hangar/logs/diario/servidor-rust");
        let deadline=std::time::Instant::now()+Duration::from_secs(10);
        while !std::fs::read_dir(&diary).into_iter().flatten().flatten().any(|file|std::fs::read_to_string(file.path()).unwrap_or_default().contains("\"sessao.estacionada\"")) {
            assert!(std::time::Instant::now()<deadline,"o estacionamento vai ao diário");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        server.abort();

        // O lançamento pelo spec grava o cano novo no arquivo da sessão (`launch_if_needed`); aqui o cano
        // novo já está de pé, para não subir `claude` nem `systemd-run` de verdade.
        let (second,escuta,lines,server)=park_cano(home.path(),&key).await;
        let mut relaunched=sidecar.clone();
        relaunched["cano"]=json!({"pid":second.0.id(),"escuta":escuta,"token":"fixture","versao":2});
        std::fs::write(dir.join("cl.json"),relaunched.to_string()).unwrap();
        processes.canos.lock().unwrap().push(i64::from(second.0.id()));
        let crate::session_write::Write::Rust(pass,target)=crate::session_write::route_write(&registry,"cl",crate::session_write::WriteRoute::Input,Duration::from_secs(5)).await
            else {panic!("a mensagem seguinte reabre a sessão no Rust")};
        // O cano falso não confirma a escrita: o envio fica esperando, e o que conta é a linha que chegou.
        let send=tokio::spawn(async move {
            let _pass=pass;
            target.handle.command(RuntimeCommand {operation_id:"oi".into(),kind:OperationKind::Input,payload:json!({"text":"oi-depois-de-estacionar","pre_transcript":false})}).await
        });
        let delivered=||lines.lock().unwrap().iter().filter(|line|line.contains("oi-depois-de-estacionar")).count();
        let deadline=std::time::Instant::now()+Duration::from_secs(10);
        while delivered()==0 {
            assert!(std::time::Instant::now()<deadline,"a mensagem chegou ao processo novo");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(delivered(),1,"entregue uma vez");
        send.abort();
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Texto que nasce no Python (orq, recibo, recado) chega pela porta privada com chave e geração: na sessão
    /// estacionada ele reabre pelo arquivo e é entregue uma vez; um envio em curso impede estacionar.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn a_private_submit_reopens_a_parked_claude_and_one_in_flight_blocks_parking() {
        let home=tempfile::tempdir().unwrap();
        let key="cd".repeat(16);
        let (first,escuta,_,server)=park_cano(home.path(),&key).await;
        let sidecar=json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":first.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        let dir=home.path().join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cl.json"),sidecar.to_string()).unwrap();
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(first.0.id())]),dir:home.path().into(),agent:Default::default()});
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned())].into());
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into()).with_processes(processes.clone()).with_codex_environment(env));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        registry.open_claude_headless("cl",false).await.unwrap().unwrap();

        {
            let barrier=registry.barrier(&key).await;
            let _in_flight=barrier.park.read().await;
            registry.park_idle_claude(Duration::ZERO).await;
            assert!(registry.has_entry(Some(&key),"cl").await,"envio em curso pela porta privada impede estacionar");
        }
        registry.park_idle_claude(Duration::ZERO).await;
        assert!(!registry.has_entry(None,"cl").await,"estacionada");
        server.abort();

        let (second,escuta,lines,server)=park_cano(home.path(),&key).await;
        let mut relaunched=sidecar.clone();
        relaunched["cano"]=json!({"pid":second.0.id(),"escuta":escuta,"token":"fixture","versao":2});
        std::fs::write(dir.join("cl.json"),relaunched.to_string()).unwrap();
        processes.canos.lock().unwrap().push(i64::from(second.0.id()));
        let envelope=Envelope {protocol:0,instance:"instance".into(),key:key.clone(),generation:1,operation_id:"orq".into(),
            clock:ClockSample {monotonic_s:0.0,epoch_s:0.0},command:json!({"kind":"submit","text":"orq-depois-de-estacionar","pre_transcript":false})};
        // O cano falso não confirma a escrita: o envio fica esperando, e o que conta é a linha que chegou.
        let send={let registry=registry.clone();tokio::spawn(async move {dispatch(&registry,&envelope).await})};
        let delivered=||lines.lock().unwrap().iter().filter(|line|line.contains("orq-depois-de-estacionar")).count();
        let deadline=std::time::Instant::now()+Duration::from_secs(10);
        while delivered()==0 {
            assert!(!send.is_finished(),"o envio do Python falhou: {:?}",send.is_finished());
            assert!(std::time::Instant::now()<deadline,"o texto do Python chegou ao processo novo");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(delivered(),1,"entregue uma vez");
        send.abort();
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Estacionar com o agente que não morre: o erro aparece uma vez e a sessão sai do Rust, em vez de ficar
    /// com o ator parado e cada rodada seguinte dar `runtime_closed`. Enquanto o agente viver, a sessão
    /// fica perdida com código e a escrita seguinte não sobe outro Claude na mesma conversa.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn a_park_whose_agent_survives_releases_the_entry() {
        let home=tempfile::tempdir().unwrap();
        let key="ef".repeat(16);
        let (first,escuta,_,server)=park_cano(home.path(),&key).await;
        let sidecar=json!({"name":"cl","key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":first.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        let dir=home.path().join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cl.json"),sidecar.to_string()).unwrap();
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(first.0.id())]),dir:home.path().into(),agent:Default::default()});
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned())].into());
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"fixture".into(),"instance".into()).with_processes(processes.clone()).with_codex_environment(env));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        let agent=Reaped(std::process::Command::new("sleep").arg("300").spawn().unwrap());
        *processes.agent.lock().unwrap()=Some(i64::from(agent.0.id()));
        registry.open_claude_headless("cl",false).await.unwrap().unwrap();
        let error=registry.park_claude(&key,1,"cl",Duration::ZERO).await.expect_err("o agente não morreu");
        assert_eq!(error.code,"lifecycle_agent_still_alive");
        assert!(!registry.has_entry(None,"cl").await,"a sessão sai do Rust com o ator parado");
        assert!(registry.park_claude(&key,1,"cl",Duration::ZERO).await.unwrap().is_none(),"a rodada seguinte não acha o que estacionar");
        assert!(registry.runtime_lost("cl").await.is_some(),"perdida com código, não pronta para entregar");
        let refused=registry.adopt_now("cl").await.expect("é sessão Claude sem terminal daqui").expect_err("nada sobe por cima do Claude vivo");
        assert_eq!(refused.code,"lifecycle_agent_still_alive");
        assert!(!registry.has_entry(None,"cl").await,"nenhuma vida nova");
        drop(agent);
        server.abort();
    }

    /// Registro de teste sobre os processos falsos do cano, com a espera da reabertura automática curta.
    #[cfg(target_os="linux")]
    fn park_registry(home:&std::path::Path,processes:Arc<ParkProcesses>,configure:impl FnOnce(RuntimeRegistry)->RuntimeRegistry) -> (Arc<RuntimeRegistry>,crate::side::Hubs) {
        park_registry_at("127.0.0.1:9".parse().unwrap(),home,processes,configure)
    }
    #[cfg(target_os="linux")]
    fn park_registry_at(upstream:SocketAddr,home:&std::path::Path,processes:Arc<ParkProcesses>,configure:impl FnOnce(RuntimeRegistry)->RuntimeRegistry) -> (Arc<RuntimeRegistry>,crate::side::Hubs) {
        let env=crate::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(),home.to_string_lossy().into_owned())].into());
        let registry=Arc::new(configure(RuntimeRegistry::new(upstream,"fixture".into(),"instance".into()).with_processes(processes).with_codex_environment(env)));
        registry.bind_lifecycle();
        let hubs=crate::side::Hubs::default();registry.bind_hubs(hubs.downgrade());
        (registry,hubs)
    }

    /// O dono fecha pelo nome a sessão sem terminal aberta no Rust (o `DELETE` que o Python atende: o sidecar
    /// sai, depois o cano morre). A queda que o ator vê não religa: nenhum erro no canal do nome e a entrada sai.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn owner_close_does_not_schedule_respawn() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("a1".repeat(16),"cl-owner-close");
        let (mut cano,escuta,_,server)=park_cano(home.path(),&key).await;
        let sidecar=json!({"name":name,"key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":cano.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        // A reabertura lê o sidecar na pasta do ciclo de vida (a temporária, nos testes).
        let files=[home.path().join(".hangar/claude-headless"),super::super::lifecycle::claude_sidecar_dir().unwrap()].map(|dir|{
            std::fs::create_dir_all(&dir).unwrap();let file=dir.join(format!("{name}.json"));std::fs::write(&file,sidecar.to_string()).unwrap();file
        });
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(cano.0.id())]),dir:home.path().into(),agent:Default::default()});
        let (diag,reports)=diag_sink().await;
        let (registry,_hubs)=park_registry_at(diag,home.path(),processes,|registry|registry.with_respawn_base(Duration::from_millis(20)));
        registry.open_claude_headless(name,false).await.unwrap().unwrap();

        for file in &files { std::fs::remove_file(file).unwrap(); }
        cano.0.kill().unwrap();cano.0.wait().unwrap();
        server.abort();
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while registry.has_entry(None,name).await {
            let error=registry.live_sender(name).borrow().as_ref().and_then(|state|state.error.clone());
            assert!(error.is_none(),"o fechamento pelo nome virou religação com erro no canal da sessão: {error:?}");
            assert!(std::time::Instant::now()<deadline,"a entrada da sessão fechada pelo nome não saiu do Rust");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let error=registry.live_sender(name).borrow().as_ref().and_then(|state|state.error.clone());
        assert!(error.is_none(),"a sessão fechada ficou com erro no canal: {error:?}");
        // A gravação que o ator deixou de fazer (o sidecar sumiu) vai ao diário, sem virar erro.
        let absent=||reports.lock().unwrap().iter().filter(|report|report["codigo"]=="claude_sidecar_absent" && report["sessao"]==name).count();
        while absent()==0 {
            assert!(std::time::Instant::now()<deadline,"a gravação engolida não foi ao diário: {:?}",reports.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(absent(),1,"uma vez por sessão");
    }

    /// O `/internal/diag` do Python: guarda o corpo de cada aviso e responde 200.
    #[cfg(target_os="linux")]
    pub(crate) async fn diag_sink() -> (SocketAddr,Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt,BufReader};
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let reports=Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen=reports.clone();
        tokio::spawn(async move {
            while let Ok((socket,_))=listener.accept().await {
                let seen=seen.clone();
                tokio::spawn(async move {
                    let (mut reader,mut length)=(BufReader::new(socket),0usize);
                    loop {
                        let mut line=String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0)==0 {return;}
                        if line=="\r\n" {break;}
                        if let Some(value)=line.to_ascii_lowercase().strip_prefix("content-length:") {length=value.trim().parse().unwrap_or(0);}
                    }
                    let mut body=vec![0;length];
                    if reader.read_exact(&mut body).await.is_err() {return;}
                    if let Ok(report)=serde_json::from_slice::<Value>(&body) {seen.lock().unwrap().push(report);}
                    let _=reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                });
            }
        });
        (address,reports)
    }

    /// `close()` sem matar (troca de nome ou de conta pelo Python, parada do servidor) com um `claude_exit` que
    /// chega junto: a religação aceita depois do cancelamento do `close` não religa a sessão recém-fechada.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn a_respawn_admitted_after_the_close_cancelled_does_not_reopen_the_closed_session() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("c3".repeat(16),"cl-close-race");
        let (cano,escuta,accepts,_cut,_,server)=serving_cano(home.path(),&key,|_|false).await;
        let sidecar=json!({"name":name,"key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":cano.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        // O `close` sem matar deixa o sidecar: só a vaga da religação pode impedir a volta.
        let files=[home.path().join(".hangar/claude-headless"),super::super::lifecycle::claude_sidecar_dir().unwrap()].map(|dir|{
            std::fs::create_dir_all(&dir).unwrap();let file=dir.join(format!("{name}.json"));std::fs::write(&file,sidecar.to_string()).unwrap();file
        });
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(cano.0.id())]),dir:home.path().into(),agent:Default::default()});
        let (registry,_hubs)=park_registry(home.path(),processes,|registry|registry.with_respawn_base(Duration::from_millis(20)));
        registry.open_claude_headless(name,false).await.unwrap().unwrap();
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while registry.live_sender(name).borrow().as_ref().and_then(|state|state.binding.clone()).is_none() {
            assert!(std::time::Instant::now()<deadline,"o ator não publicou o vínculo");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let barrier=registry.barrier(&key).await;
        let guard=barrier.lock().await;
        // Operação de mentira: o `close` a cancela antes de esperar a barreira, e isso diz que ele já passou dali.
        let probe=Arc::new(std::sync::atomic::AtomicBool::new(false));
        *barrier.operation.lock().unwrap()=Some((name.into(),probe.clone()));
        let close={let registry=registry.clone();let key=key.clone();tokio::spawn(async move {registry.close(&key,1).await})};
        while !probe.load(std::sync::atomic::Ordering::Acquire) {
            assert!(std::time::Instant::now()<deadline,"o close não cancelou");
            tokio::task::yield_now().await;
        }
        registry.schedule_claude_respawn(&key,1).await;
        assert!(barrier.operation.lock().unwrap().as_ref().is_some_and(|(_,flag)|!Arc::ptr_eq(flag,&probe)),"a religação foi aceita depois do cancelamento");
        drop(guard);
        close.await.unwrap().unwrap();
        while barrier.operation.lock().unwrap().is_some() {
            assert!(std::time::Instant::now()<deadline,"a religação não terminou");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let error=registry.live_sender(name).borrow().as_ref().and_then(|state|state.error.clone());
        let reopened=registry.has_entry(None,name).await;
        if reopened {registry.close(&key,1).await.unwrap();}
        assert!(!reopened && error.is_none(),"a sessão fechada voltou ({reopened}) ou ficou com erro ({error:?}); conexões ao cano: {}",accepts.load(std::sync::atomic::Ordering::SeqCst));
        for file in &files {let _=std::fs::remove_file(file);}
        server.abort();
    }

    /// Cano de verdade da chave e um servidor que atende várias conexões: `refuse(n)` fecha a n-ésima (a
    /// primeira é 1) logo depois do pedido; `cut` derruba a conexão servida; `lines` anota o que chega depois do retrato.
    #[cfg(target_os="linux")]
    #[allow(clippy::type_complexity)]
    async fn serving_cano(dir:&std::path::Path,key:&str,refuse:fn(usize)->bool)
        -> (Reaped,String,Arc<std::sync::atomic::AtomicUsize>,Arc<tokio::sync::Notify>,Arc<std::sync::Mutex<Vec<String>>>,tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
        let (child,_,_,unused)=park_cano(dir,key).await;
        unused.abort();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let (accepts,cut)=(Arc::new(std::sync::atomic::AtomicUsize::new(0)),Arc::new(tokio::sync::Notify::new()));
        let lines=Arc::new(std::sync::Mutex::new(Vec::new()));
        let (counted,cutting,seen)=(accepts.clone(),cut.clone(),lines.clone());
        let server=tokio::spawn(async move {
            let mut connections=tokio::task::JoinSet::new();
            while let Ok((socket,_))=listener.accept().await {
                let n=counted.fetch_add(1,std::sync::atomic::Ordering::SeqCst)+1;
                let (cutting,seen)=(cutting.clone(),seen.clone());
                connections.spawn(async move {
                    let mut socket=BufReader::new(socket);let mut line=String::new();
                    if socket.read_line(&mut line).await.unwrap_or(0)==0 || refuse(n) {return;}
                    let snapshot=json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
                    if socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.is_err() {return;}
                    loop {
                        line.clear();
                        tokio::select! { _=cutting.notified()=>break, read=socket.read_line(&mut line)=>if read.unwrap_or(0)==0 {break} else {seen.lock().unwrap().push(line.clone())} }
                    }
                });
            }
        });
        (child,format!("tcp:{address}"),accepts,cut,lines,server)
    }

    /// Conversa aberta (`request_reopen`) numa sessão sem terminal saudável com religação marcada para
    /// daqui a uma hora: a tentativa marcada sai na hora, sem reabrir a sessão nem abrir outra tentativa.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn request_reopen_wakes_the_scheduled_reconnect_of_a_healthy_headless_claude() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("b2".repeat(16),"cl-reopen-wake");
        let (cano,escuta,accepts,cut,_,server)=serving_cano(home.path(),&key,|n|n==2).await;
        let dir=home.path().join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")),json!({"name":name,"key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,
            "permission_mode":"manual","cano":{"pid":cano.0.id(),"escuta":escuta,"token":"fixture","versao":2}}).to_string()).unwrap();
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(cano.0.id())]),dir:home.path().into(),agent:Default::default()});
        let hour=Duration::from_secs(3600);
        let (registry,_hubs)=park_registry(home.path(),processes,|registry|registry.with_reconnect_waits(vec![Duration::ZERO,hour],vec![Duration::ZERO,hour],hour));
        registry.open_claude_headless(name,false).await.unwrap().unwrap();
        let accepted=||accepts.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(accepted(),1);

        cut.notify_one();
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while accepted()<2 {
            assert!(std::time::Instant::now()<deadline,"a primeira tentativa (espera zero) não saiu");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // A recusa marca a próxima para daqui a uma hora; o pedido só antecipa a marcada, então repeti-lo é inofensivo.
        while accepted()<3 {
            assert!(std::time::Instant::now()<deadline,"a conversa aberta não antecipou a religação marcada");
            registry.request_reopen(name);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(accepted(),3,"uma tentativa só, sem outra aberta pelo pedido");
        let (generation,handle)=registry.entries.lock().await.get(&key).map(|entry|(entry.generation,entry.handle.clone())).expect("a sessão segue aberta");
        assert_eq!(generation,1,"religar não é reabrir");
        let snapshot=handle.snapshot().await.unwrap();
        assert!(snapshot["error"].is_null(),"religada sem erro: {snapshot}");
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// D2 vale para o Codex sem terminal também: abrir a conversa acorda a religação marcada do ator dele.
    #[tokio::test]
    async fn request_reopen_wakes_the_reconnect_of_a_headless_codex() {
        let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into()));registry.bind_lifecycle();
        let (handle,wakes)=super::super::actor::RuntimeHandle::counting_wakes("CX",1);
        let binding=crate::interactions::InteractionBinding {key:"CX".into(),conversation:"thread".into(),generation:1};
        let dir=tempfile::tempdir().unwrap();
        registry.entries.lock().await.insert("CX".into(),Entry {generation:1,handle:EntryHandle::Headless(handle),lease_path:dir.path().join("CX.lock"),name:"cx-wake".into(),mods_life:0,provider:"codex".into(),account_key:None,relaunch:None,settings_owner:None,
            mode_source:crate::terminal_facts::ModeSource {binding,transcript:"t.jsonl".into(),confirmed:None},live:registry.live_sender("cx-wake"),runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
        registry.request_reopen("cx-wake");
        tokio::time::timeout(Duration::from_secs(5),async {while wakes.load(std::sync::atomic::Ordering::SeqCst)==0 {tokio::time::sleep(Duration::from_millis(10)).await;}})
            .await.expect("a conversa aberta não acordou a religação do Codex");
    }

    /// Sessão sem terminal com o cano vivo da chave (`refuse` decide quais conexões ele recusa) e a subida do
    /// servidor sobre ela.
    #[cfg(target_os="linux")]
    #[allow(clippy::type_complexity)]
    async fn boot_over_live_cano(home:&std::path::Path,key:&str,name:&str,initialized:bool,refuse:fn(usize)->bool,configure:impl FnOnce(RuntimeRegistry)->RuntimeRegistry)
        -> (Reaped,Arc<std::sync::atomic::AtomicUsize>,Arc<std::sync::Mutex<Vec<String>>>,tokio::task::JoinHandle<()>,Arc<RuntimeRegistry>,crate::side::Hubs) {
        let (cano,escuta,accepts,_cut,lines,server)=serving_cano(home,key,refuse).await;
        let dir=home.join(".hangar/claude-headless");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")),json!({"name":name,"key":key,"cwd":home,"session_id":"a","headless":true,"initialized":initialized,
            "permission_mode":"manual","cano":{"pid":cano.0.id(),"escuta":escuta,"token":"fixture","versao":2}}).to_string()).unwrap();
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(cano.0.id())]),dir:home.into(),agent:Default::default()});
        let (registry,hubs)=park_registry(home,processes,configure);
        registry.clone().register_claude_headless().await;
        (cano,accepts,lines,server,registry,hubs)
    }

    /// Mensagem pelo caminho do app (`route_write`), sem esperar a confirmação que o cano falso não manda.
    #[cfg(target_os="linux")]
    async fn send_text(registry:&Arc<RuntimeRegistry>,name:&str,id:&str,text:&str) -> Result<RuntimeReply,RuntimeError> {
        let crate::session_write::Write::Rust(pass,target)=crate::session_write::route_write(registry,name,crate::session_write::WriteRoute::Input,Duration::from_secs(5)).await
            else {panic!("a escrita não chegou ao ator do Rust")};
        let _pass=pass;
        let command=RuntimeCommand {operation_id:id.into(),kind:OperationKind::Input,payload:json!({"text":text,"pre_transcript":false})};
        tokio::time::timeout(Duration::from_millis(500),target.handle.command(command)).await.unwrap_or_else(|_|Err(failure("fixture_no_ack")))
    }

    /// Subida com o cano vivo da chave que recusa a primeira conexão: a sessão nasce com ator, saudável e sem
    /// aviso, religa na tentativa seguinte e entrega o que chega depois.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn boot_with_live_unreachable_cano_keeps_retrying() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("d4".repeat(16),"cl-boot-retry");
        let (_cano,accepts,lines,server,registry,_hubs)=boot_over_live_cano(home.path(),&key,name,true,|n|n==1,|registry|registry).await;
        let handle=registry.handle(&key,1).await.expect("cano vivo na subida nunca fica sem ator");
        let snapshot=handle.snapshot().await.unwrap();
        assert!(snapshot["error"].is_null() && snapshot["view"]["alive"]!=false,"nasce saudável: {snapshot}");
        assert!(registry.runtime_lost(name).await.is_none(),"sem código de sessão perdida");
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while accepts.load(std::sync::atomic::Ordering::SeqCst)<2 {
            assert!(std::time::Instant::now()<deadline,"a religação não tentou de novo");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let view=handle.view().await.unwrap();
        assert!(view["public_state"]["problema"].is_null(),"religar não mostra aviso: {view}");
        let _=send_text(&registry,name,"depois","oi-depois-da-subida").await;
        let delivered=||lines.lock().unwrap().iter().filter(|line|line.contains("oi-depois-da-subida")).count();
        while delivered()==0 {
            assert!(std::time::Instant::now()<deadline,"a sessão religada não entregou");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(delivered(),1,"entregue uma vez");
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst),2,"uma religação só");
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Escrita que chega na subida antes da primeira conexão ao cano vivo: espera na fila e sai quando ele atende.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn boot_write_before_the_first_connection_waits_and_is_delivered() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("e5".repeat(16),"cl-boot-write");
        let hour=Duration::from_secs(3600);
        let (_cano,accepts,lines,server,registry,_hubs)=boot_over_live_cano(home.path(),&key,name,true,|n|n<=2,
            |registry|registry.with_reconnect_waits(vec![Duration::ZERO,hour],vec![Duration::ZERO,hour],hour)).await;
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while accepts.load(std::sync::atomic::Ordering::SeqCst)<2 {
            assert!(std::time::Instant::now()<deadline,"a primeira tentativa do ator não saiu");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let delivered=||lines.lock().unwrap().iter().filter(|line|line.contains("oi-antes-de-conectar")).count();
        let reply=send_text(&registry,name,"antes","oi-antes-de-conectar").await.expect("a escrita entra no ator");
        assert_eq!(reply.disposition,Disposition::Deferred,"sem conexão, fica na fila: {reply:?}");
        while delivered()==0 {
            assert!(std::time::Instant::now()<deadline,"a mensagem da fila não saiu ao conectar");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(delivered(),1,"entregue uma vez");
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst),3,"a mensagem antecipou a tentativa marcada");
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Sessão que ainda não tinha inicializado e cujo cano não atendeu na subida: o `initialize` sai na
    /// primeira conexão, uma vez.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn boot_without_initialization_initializes_on_the_first_connection() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("f6".repeat(16),"cl-boot-init");
        let (_cano,accepts,lines,server,registry,_hubs)=boot_over_live_cano(home.path(),&key,name,false,|n|n==1,|registry|registry).await;
        let initializes=||lines.lock().unwrap().iter().filter(|line|line.contains("initialize")).count();
        let deadline=std::time::Instant::now()+Duration::from_secs(20);
        while initializes()==0 {
            assert!(std::time::Instant::now()<deadline,"o initialize não saiu ao conectar (conexões: {})",accepts.load(std::sync::atomic::Ordering::SeqCst));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(initializes(),1,"uma vez");
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst),2);
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Ator sem conexão não leu o retrato (turno, perguntas): parado há qualquer tempo, mesmo além do
    /// `PARK_AFTER`, não é estacionado, senão o vigia mataria um turno vivo.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn boot_unconnected_actor_idle_past_park_after_is_not_parked() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("a7".repeat(16),"cl-boot-park");
        let (cano,accepts,_lines,server,registry,_hubs)=boot_over_live_cano(home.path(),&key,name,true,|_|true,|registry|registry).await;
        assert!(registry.has_entry(Some(&key),name).await,"nasce com ator");
        // `ZERO` = qualquer tempo parado conta como acima do prazo.
        registry.park_idle_claude(Duration::ZERO).await;
        assert!(registry.has_entry(Some(&key),name).await,"sem conexão a sessão não é estacionada");
        assert!(process::probe_alive(cano.0.id()),"o cano segue vivo (conexões: {})",accepts.load(std::sync::atomic::Ordering::SeqCst));
        registry.close(&key,1).await.unwrap();
        server.abort();
    }

    /// Abertura sem adoção do cano que não atende (a reabertura que acabou de subir um cano novo): falha na
    /// hora e não deixa ator, para quem subiu o cano desfazer.
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn open_of_a_just_launched_cano_that_refuses_fails_at_once() {
        let home=tempfile::tempdir().unwrap();
        let (key,name)=("b8".repeat(16),"cl-open-fresh");
        let (cano,escuta,_accepts,_cut,_lines,server)=serving_cano(home.path(),&key,|_|true).await;
        let processes=Arc::new(ParkProcesses {canos:std::sync::Mutex::new(vec![i64::from(cano.0.id())]),dir:home.path().into(),agent:Default::default()});
        let (registry,_hubs)=park_registry(home.path(),processes,|registry|registry);
        let queue=home.path().join("queue");
        let meta=json!({"name":name,"key":key,"cwd":home.path(),"session_id":"a","headless":true,"initialized":true,"permission_mode":"manual",
            "cano":{"pid":cano.0.id(),"escuta":escuta,"token":"fixture","versao":2}});
        let target=RuntimeTarget {key:key.clone(),generation:1,name:name.into(),provider:"claude".into(),metadata:meta,
            binding:CanoBinding {pid:cano.0.id(),escuta,token:"fixture".into(),versao:2},lease_path:queue.join("lease"),state_path:queue.join("state.json"),
            projection_dir:queue.clone(),transcript:home.path().join("a.jsonl"),created:0.0};
        std::fs::create_dir_all(&queue).unwrap();
        let opened=tokio::time::timeout(Duration::from_secs(30),registry.open_headless_locked(target,None,false,false)).await.expect("a abertura não espera");
        assert!(opened.is_err(),"cano novo que não atende é falha da abertura: {opened:?}");
        assert!(!registry.has_entry(Some(&key),name).await,"sem ator para o cano recém-subido");
        server.abort();
    }
}
