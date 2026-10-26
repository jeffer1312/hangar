//! Relançar o Codex já aberto, do começo ao fim no Rust: troca terminal ↔ sem terminal e reiniciar o sem
//! terminal. A troca roda sob a exclusão da vigia (nenhuma rodada enterra a vida, e com ela a fila, no
//! meio) e com a porta de entrada da sessão retida: escrita durante a troca recebe 409 na hora.
//! Modo de permissão que não se lê no seletor recusa a troca; nunca vira Full Access.
use super::actor::{RuntimeHandle,TURN_HOLD_BUSY};
use super::codex_sidecar::{self,Meta};
use super::codex_terminal::{self as terminal,Watcher};
use super::gateway::{EntryHandle,RuntimeRegistry};
use super::ingress::IngressGates;
use super::protocol::{CanoBinding,ClockSample,Disposition,OperationKind,RuntimeCommand,RuntimeError,RuntimeTarget};
use super::queue::{Action,State as QueueState,Store};
use crate::accounts::{AccountService,Provider,catalog::resolved};
use crate::list::discover_other::truthy;
use crate::list::procs::{ProcessView,SystemProcs,exited};
use crate::mods::state::random_hex;
use crate::permission_control::PermissionAction;
use crate::state::capture::{MuxProcess,TIMEOUT};
use crate::session_open::terminal::PaneSpec;
use serde_json::{Value,json};
use std::path::{Path,PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Os modos que o sem terminal sabe subir (`sem_terminal.MODOS`).
const SUPPORTED:[&str;3] = ["Ask for approval","Approve for me","Full Access"];
/// Escrita em curso que a porta espera terminar antes de reter.
const INGRESS_WAIT:Duration = Duration::from_secs(10);
/// O `_esperar_saida` do Python.
const EXIT_WAIT:Duration = Duration::from_secs(5);
/// Motor novo até assentar na conversa.
const SETTLE_WAIT:Duration = Duration::from_secs(45);
/// Relançamentos seguidos do sidecar antigo antes de a escrita parar de tentar, e por quanto tempo.
const LEGACY_TRIES:u8 = 3;
const LEGACY_WINDOW:Duration = Duration::from_secs(600);
/// Releitura da configuração da conversa antes de ir para sem terminal.
const SETTINGS_READ_WAIT:Duration = Duration::from_secs(5);
const POLL:Duration = Duration::from_millis(200);
/// Fases da transferência que já terminaram (`_TERMINAL`).
const TRANSFER_DONE:[&str;3] = ["complete","rejected","rolled_back"];

const MSG_UNSUPPORTED:&str = "Esta política de permissões não é suportada sem terminal; a sessão foi mantida.";
const MSG_ACCOUNT:&str = "A conta Codex mudou; a conversa não foi transferida.";
const MSG_QUEUE:&str = "Há mensagens na fila esperando entrega.";

/// Como a troca para sem terminal lê o modo de permissão atual.
pub enum PermissionRead {
    /// O seletor `/permissions` da TUI, com o painel de terminal real (aberto recusa).
    Pane(Option<Arc<crate::term::Terms>>),
    /// Só testes: o modo já lido, sem a TUI.
    #[doc(hidden)]
    Known(String),
}

/// Recusa antes ou durante a troca com a frase do Python; a rota responde 409 `erro_troca_modo`.
fn refuse(message:&str) -> RuntimeError { RuntimeError::new("erro_troca_modo",message) }
fn busy() -> RuntimeError { RuntimeError::new("session_transfer_busy","a sessão está trocando de agente") }
fn gone() -> RuntimeError { refuse("A sessão Codex não está mais aberta aqui.") }
fn job() -> RuntimeError { RuntimeError::new("relaunch_job","etapa da troca de modo interrompida") }
/// O terminal recém-criado ainda abre a conversa: trocar de modo espera ele.
pub const STARTING:&str = "codex_terminal_starting";
pub fn starting() -> RuntimeError { RuntimeError::new(STARTING,"Espere o terminal abrir a conversa antes de trocar de modo.") }
/// Sem sidecar, só o pane do Codex é a TUI abrindo; pane de outro agente com o nome é dele.
fn codex_opening(panes:&[crate::list::mux::Pane],name:&str) -> bool {
    terminal::pane_of(panes,name).is_some_and(|pane|pane.provider.as_deref() == Some("codex"))
}

/// Falha depois de mexer em processo, sem volta automática: a rota responde 500 com o código.
pub fn kill_phase(error:&RuntimeError) -> bool { error.code.starts_with("relaunch_kill") || error.code == "relaunch_process_alive" }

fn supported(mode:Option<&str>) -> Option<&'static str> {
    let mode = mode?.trim();
    SUPPORTED.into_iter().find(|name|name.eq_ignore_ascii_case(mode))
}

fn clock() -> ClockSample {
    ClockSample { monotonic_s:0.0,epoch_s:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0) }
}

/// Fechamento retido da porta, desfeito ao sair por qualquer caminho.
struct Held<'a> { gates:&'a IngressGates, name:&'a str }
impl Drop for Held<'_> { fn drop(&mut self) { self.gates.release(self.name); } }

async fn hold<'a>(gates:&'a IngressGates,name:&'a str) -> Result<Held<'a>,RuntimeError> {
    gates.hold(name,INGRESS_WAIT).await.map_err(|_|busy())?;
    Ok(Held { gates,name })
}

/// Transferência de conversa sem fase final para este nome: pelo `transfer_id` do sidecar ou pelo índice
/// por sessão. Pelo índice só vale o registro desta vida (`source_life` = `k:<chave>`, o `_belongs` do
/// Python): o de outra vida de mesmo nome não segura a troca. Registro ilegível conta como em curso.
pub(crate) fn transfer_open(home:&Path,name:&str,sidecar_transfer:Option<&str>,key:Option<&str>) -> bool {
    let base = home.join(".hangar").join("conversation-transfers");
    let open = |id:&str,marked:bool| -> bool {
        let id = id.to_ascii_lowercase();
        if id.len() != 36 || !id.bytes().all(|b|b.is_ascii_hexdigit() || b == b'-') { return true; }
        let raw = match std::fs::read(base.join(format!("{id}.json"))) {
            Ok(raw)=>raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound=>return false,
            Err(_)=>return true,
        };
        serde_json::from_slice::<Value>(&raw).map_or(true,|record|record["name"] == name && !TRANSFER_DONE.contains(&record["phase"].as_str().unwrap_or(""))
            && (marked || key.is_none_or(|key|record["source_life"] == format!("k:{key}"))))
    };
    if sidecar_transfer.is_some_and(|id|open(id,true)) { return true; }
    let digest = ring::digest::digest(&ring::digest::SHA256,name.as_bytes());
    let hex:String = digest.as_ref().iter().map(|b|format!("{b:02x}")).collect();
    match std::fs::read(base.join("indexes").join(format!("session-{hex}.json"))) {
        Ok(raw)=>serde_json::from_slice::<Vec<String>>(&raw).map_or(true,|ids|ids.iter().any(|id|open(id,false))),
        Err(error)=>error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// (pid, nascimento) das raízes e de todos os descendentes vivos agora (o `_descendant_pids` do Python:
/// no Windows o `app_pid` pode ser o shim `cmd.exe`). O que já saiu não entra; leitura que não prova
/// nem vida nem saída é `Err`, e quem chama não segue às cegas.
async fn births(roots:Vec<i64>) -> Result<Vec<(i64,f64)>,()> {
    let roots:Vec<i64> = roots.into_iter().filter(|pid|*pid > 1).collect();
    if roots.is_empty() { return Ok(Vec::new()); }
    tokio::task::spawn_blocking(move || {
        let procs = SystemProcs::default();
        let children = procs.children(Duration::ZERO).map_err(|_|())?;
        let mut pids:Vec<i64> = roots.into_iter().flat_map(|root|crate::list::discover::descendants(root,&children)).collect();
        pids.sort_unstable();
        pids.dedup();
        let mut lives = Vec::new();
        for pid in pids {
            match procs.recipe(pid) {
                Ok(recipe)=>lives.push((pid,recipe.birth)),
                Err(error) if exited(&error)=>{},
                // O critério do `process_absent`: sem `/proc` e fora da árvore relida, saiu.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound=>{
                    if procs.children(Duration::ZERO).map_err(|_|())?.values().any(|kids|kids.contains(&pid)) { return Err(()); }
                }
                Err(_)=>return Err(()),
            }
        }
        Ok(lives)
    }).await.map_err(|_|())?
}

/// `wait_absent`; uma raiz conhecida (`roots`: app, TUI, lançador, pane, cano) que passa do prazo leva o
/// encerramento (SIGTERM; árvore no Windows) e ganha outro prazo. Descendente só é esperado, nunca
/// sinalizado: pode ser um processo que a sessão deixou de pé de propósito (servidor com `nohup`).
/// `Err` traz os pids ainda vivos.
async fn ensure_absent(lives:Vec<(i64,f64)>,roots:&[i64]) -> Result<(),Vec<i64>> {
    let left = wait_absent(lives.clone()).await;
    if left.is_empty() { return Ok(()); }
    let survivors:Vec<(i64,f64)> = lives.iter().copied().filter(|(pid,_)|left.contains(pid) && roots.contains(pid)).collect();
    if !survivors.is_empty() {
        let signalled = tokio::task::spawn_blocking(move || {
            let procs = SystemProcs::default();
            for (pid,birth) in survivors {
                // Nascimento conferido logo antes do sinal: pid reaproveitado por outro processo não o recebe.
                if !matches!(super::lifecycle::process_absent(&procs,pid,birth),Ok(false)) { continue; }
                if u32::try_from(pid).map_or(true,|pid|super::process::terminate(pid).is_err()) {
                    tracing::warn!(pid,"processo antigo da troca de modo não recebeu o encerramento");
                }
            }
        }).await;
        if signalled.is_err() { return Err(left); }
    }
    let left = wait_absent(lives).await;
    if left.is_empty() { Ok(()) } else { Err(left) }
}

/// O 500 de processo antigo vivo, com os pids que seguram a troca.
fn alive_error(message:&str,pids:&[i64]) -> RuntimeError {
    let pids:Vec<String> = pids.iter().map(i64::to_string).collect();
    RuntimeError::new("relaunch_process_alive",&format!("{message} Ainda vivos: {}.",pids.join(", ")))
}

/// Os pids que não saíram dentro do prazo (vazio = todos saíram). Fonte ilegível não prova saída.
async fn wait_absent(lives:Vec<(i64,f64)>) -> Vec<i64> {
    let all:Vec<i64> = lives.iter().map(|(pid,_)|*pid).collect();
    tokio::task::spawn_blocking(move || {
        let procs = SystemProcs::default();
        let deadline = std::time::Instant::now() + EXIT_WAIT;
        loop {
            let left:Vec<i64> = lives.iter().filter(|(pid,birth)|!matches!(super::lifecycle::process_absent(&procs,*pid,*birth),Ok(true)))
                .map(|(pid,_)|*pid).collect();
            if left.is_empty() || std::time::Instant::now() >= deadline { return left; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }).await.unwrap_or(all)
}

/// O motor novo assentou (`field`) na conversa de antes; só então o modo de colaboração volta.
async fn settled(handle:&RuntimeHandle,field:&str,thread:&str) -> Result<(),RuntimeError> {
    let deadline = tokio::time::Instant::now() + SETTLE_WAIT;
    loop {
        let view = handle.view().await?;
        if view[field] == true {
            if view["thread_id"].as_str() == Some(thread) { return Ok(()); }
            return Err(RuntimeError::new("relaunch_thread","O Codex não retomou a conversa original."));
        }
        if field == "ready" && view["alive"] == false { return Err(RuntimeError::new("relaunch_engine_exited","O Codex não respondeu.")); }
        if tokio::time::Instant::now() >= deadline { return Err(RuntimeError::new("relaunch_settle_timeout","o Codex não retomou a conversa em 45 segundos")); }
        tokio::time::sleep(POLL).await;
    }
}

/// Só depois do `settled` (motor na mesma conversa). "Aceito" não basta: a resposta do app-server traz o
/// modo que vale depois da troca (gravado no sidecar pelo `session.patch_meta` do mesmo passo), e a vista
/// relida tem de confirmá-lo.
async fn set_mode(handle:&RuntimeHandle,mode:&str) -> Result<(),RuntimeError> {
    let lost = ||RuntimeError::new("relaunch_mode","o modo de colaboração não voltou depois da troca");
    let reply = handle.command(RuntimeCommand { operation_id:random_hex(16),kind:OperationKind::SetMode,payload:json!({"mode":mode}) }).await?;
    if reply.disposition != Disposition::Accepted || reply.payload["mode"].as_str() != Some(mode) { return Err(lost()); }
    if handle.view().await?["mode"].as_str() != Some(mode) { return Err(lost()); }
    Ok(())
}

/// O modo de permissão que o processo novo leu é o que a troca gravou (o sidecar é a fonte durável dele).
fn same_permission(read:&Meta,wanted:&Meta) -> Result<(),RuntimeError> {
    if wanted.get("permission_mode").is_some_and(|mode|read.get("permission_mode") == Some(mode)) { return Ok(()); }
    Err(RuntimeError::new("relaunch_permission","o modo de permissão gravado mudou durante a troca"))
}

/// Volta que também falhou: processo da volta que não saiu mantém o código dele (500); o resto é a recusa.
fn restored_failure(message:&str,restore:&RuntimeError) -> RuntimeError {
    if kill_phase(restore) { RuntimeError::new(&restore.code,message) } else { refuse(message) }
}

/// Leitura de processo que não prova saída, antes de mexer em processo: nada foi encerrado.
fn unread() -> RuntimeError { refuse("Não consegui ler os processos da sessão; nada foi encerrado.") }

/// Sidecar com os campos de processo zerados no modo pedido.
fn cleared(mut meta:Meta,headless:bool) -> Meta {
    meta.insert("headless".into(),json!(headless));
    for field in ["cano","endpoint","app_pid","tui_pid","launcher_pid"] { meta.insert(field.into(),Value::Null); }
    meta
}

/// Modelo, esforço e Fast valem o que o motor tem agora, não o que a abertura gravou.
fn with_settings(mut meta:Meta,view:&Value,key:&str) -> Meta {
    for field in ["model","effort","service_tier"] { if !view[field].is_null() { meta.insert(field.into(),view[field].clone()); } }
    meta.insert("key".into(),json!(key));
    meta
}

/// Sidecar do terminal: o lançador não aceita modo nulo, então o padrão da abertura vai gravado;
/// texto desconhecido recusa aqui, antes de tocar em processo.
fn terminal_settings(meta:Meta,view:&Value,key:&str) -> Result<Meta,RuntimeError> {
    let mode = super::codex_launch::stored_mode(&Value::Object(meta.clone()))?;
    let mut settings = with_settings(meta,view,key);
    settings.insert("permission_mode".into(),json!(mode));
    Ok(settings)
}

/// Plan/default voltam depois da troca; outro modo não existe no sem terminal.
fn collaboration(view:&Value) -> Option<String> { view["mode"].as_str().filter(|mode|matches!(*mode,"plan" | "default")).map(str::to_owned) }

/// Alvo do sem terminal a partir do sidecar e dos caminhos da fila (o `Binding.descriptor()` do Python).
fn headless_target(queue_dir:&Path,name:&str,key:&str,generation:u64,meta:&Meta) -> RuntimeTarget {
    let (state_path,lease_path) = terminal::queue_files(queue_dir,key);
    let mut metadata = Value::Object(meta.clone());
    metadata["key"] = json!(key);
    RuntimeTarget { key:key.into(),generation,name:name.into(),provider:"codex".into(),
        binding:CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 },
        transcript:terminal::text(meta,"rollout_path").map(PathBuf::from).unwrap_or_default(),
        created:meta.get("created").and_then(Value::as_f64).unwrap_or(0.0),
        metadata,lease_path,state_path,projection_dir:queue_dir.to_owned() }
}

impl Watcher {
    async fn load(&self,name:&str) -> Option<Meta> {
        let file = codex_sidecar::path(&codex_sidecar::dir(&self.cfg.home),name);
        tokio::task::spawn_blocking(move||codex_sidecar::load(&file)).await.ok().flatten()
    }

    async fn transfer_busy(&self,name:&str,meta:&Meta) -> bool {
        let (home,name,transfer) = (self.cfg.home.clone(),name.to_owned(),terminal::text(meta,"transfer_id").map(str::to_owned));
        let key = terminal::text(meta,"key").map(str::to_owned);
        tokio::task::spawn_blocking(move||transfer_open(&home,&name,transfer.as_deref(),key.as_deref())).await.unwrap_or(true)
    }

    async fn sidecar_update(&self,name:&str,expect:Vec<(&'static str,Value)>,patch:Value) -> Result<Option<Meta>,RuntimeError> {
        let (dir,name) = (codex_sidecar::dir(&self.cfg.home),name.to_owned());
        let patch = patch.as_object().cloned().unwrap_or_default();
        tokio::task::spawn_blocking(move||codex_sidecar::update_if(&dir,&name,&expect,&patch)).await.map_err(|_|job())?.map_err(terminal::io_failure)
    }

    async fn sidecar_put(&self,name:&str,meta:&Meta) -> Result<(),RuntimeError> {
        let (dir,name,meta) = (codex_sidecar::dir(&self.cfg.home),name.to_owned(),meta.clone());
        tokio::task::spawn_blocking(move||codex_sidecar::put(&dir,&name,&meta)).await.map_err(|_|job())?.map_err(terminal::io_failure)
    }

    /// Desfaz o `app_pid` nulo da vida que ficou: só sobre o sidecar ainda estacionado por esta troca.
    async fn unpark(&self,name:&str,key:&str,app_pid:u32) -> bool {
        let expect = vec![("key",json!(key)),("app_pid",Value::Null)];
        match self.sidecar_update(name,expect,json!({"app_pid":app_pid})).await {
            Ok(Some(_))=>true,
            Ok(None)=>{ tracing::warn!(session=%name,"sidecar mudou de vida; app_pid não voltou"); false },
            Err(error)=>{ tracing::warn!(session=%name,code=%error.code,"app_pid não voltou ao sidecar"); false },
        }
    }

    /// Geração em que a fila da chave está agora, sem mexer nela.
    async fn generation(&self,key:&str,name:&str) -> Result<u64,RuntimeError> {
        let (state_path,lease_path) = terminal::queue_files(&self.cfg.queue_dir,key);
        let (queue_dir,key,name) = (self.cfg.queue_dir.clone(),key.to_owned(),name.to_owned());
        tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
            let _lease = terminal::lease_blocking(&lease_path)?;
            Ok(Store::open(&state_path,&queue_dir,QueueState::new(&key,1,&name,Vec::new()))?.state().generation)
        }).await.map_err(|_|job())?.map_err(terminal::io_failure)
    }

    /// Na subida: abre cada Codex sem terminal com cano vivo da chave ou com mensagem esperando na fila.
    /// O parado sem pendência fica fechado; o próximo envio o abre (`open_parked`).
    pub async fn register_codex_headless(&self,registry:&RuntimeRegistry) {
        let (dir,queue)=(codex_sidecar::dir(&self.cfg.home),self.cfg.queue_dir.clone());
        let listed=tokio::task::spawn_blocking(move||{
            let mut problems=crate::list::discover::Problems::default();
            crate::list::discover_other::sidecars(&dir,"sidecar Codex ilegível ou torto",&mut problems).iter().filter_map(|(_,meta)|{
                if !meta.get("headless").is_some_and(truthy) {return None;}
                let (name,key)=(terminal::text(&meta,"name")?.to_owned(),terminal::text(&meta,"key")?.to_owned());
                let alive=meta.get("cano").and_then(|cano|cano["pid"].as_u64()).and_then(|pid|u32::try_from(pid).ok())
                    .is_some_and(|pid|super::process::liveness(pid,&key)==super::process::Liveness::Ours);
                (alive || super::gateway::pending(&terminal::queue_files(&queue,&key).0)).then_some(name)
            }).collect::<Vec<_>>()
        }).await.unwrap_or_default();
        let opens=listed.into_iter().map(|name|async move {
            if let Some(Err(error))=self.open_parked(registry,&name).await {
                tracing::warn!(session=%name,code=%error.code,"Codex sem terminal não abriu na subida");
                registry.report("runtime.registration_failed",&name,&error.code,"o Codex sem terminal não abriu na subida do Rust");
            }
        });
        futures_util::future::join_all(opens).await;
    }

    /// Codex sem terminal parado (sem ator aqui, cano morto ou vivo): o Rust é o dono e o abre na hora,
    /// subindo o processo se preciso, como o `ensure_running` do Python. `None`: não é sem terminal do
    /// Rust, ou há transferência a recuperar (o Python atende como hoje).
    pub async fn open_parked(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<(),RuntimeError>> {
        use crate::session_write::{Owner,Provider as Route,WriteRoute,decide};
        let meta = self.load(name).await?;
        if !meta.get("headless").is_some_and(truthy) || decide(WriteRoute::CodexControl,Route::Codex,false) != Owner::Rust { return None; }
        let key = terminal::text(&meta,"key")?.to_owned();
        if registry.codex_headless(name).await.is_some() { return Some(Ok(())); }
        // Outra entrada com esta chave ou nome (vida de terminal ainda aberta, outro provedor): é do Rust e
        // não dá para abrir; a escrita recebe o código, nunca o Python.
        if registry.has_entry(Some(&key),name).await {
            return Some(Err(RuntimeError::new("codex_headless_entry_conflict","já há outra sessão aberta aqui com este nome ou chave")));
        }
        // Transferência a recuperar: é do Python, como hoje.
        if self.transfer_busy(name,&meta).await { return None; }
        let generation = match self.generation(&key,name).await { Ok(generation)=>generation, Err(error)=>return Some(Err(error)) };
        let mut target = headless_target(&self.cfg.queue_dir,name,&key,generation,&meta);
        // Cano gravado e vivo é reaproveitado (`launch_if_needed`); sem ele, sobe um.
        if let Some(cano) = meta.get("cano").cloned().and_then(|cano|serde_json::from_value::<super::process::Cano>(cano).ok()) {
            target.binding = CanoBinding { pid:cano.pid,escuta:cano.escuta,token:cano.token,versao:cano.versao };
        }
        Some(registry.open_with_launch(target,codex_sidecar::dir(&self.cfg.home)).await.map(|_|()))
    }

    /// Sidecar do desenho antigo (terminal sem `endpoint`): sem pane, relança TUI e app-server pelo lançador
    /// (`--cwd`/`--resume` + chave), adota e entrega, como o `ensure_running` do Python. Pane vivo nunca é
    /// recriado; três falhas seguidas param as tentativas por escrita. Falha sai com o código dela.
    pub(super) async fn relaunch_legacy(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<(),RuntimeError>> {
        let meta = self.load(name).await?;
        if meta.get("headless").is_some_and(truthy) || terminal::text(&meta,"endpoint").is_some() || self.transfer_busy(name,&meta).await { return None; }
        let given_up = {
            let mut failures = self.legacy_failures.lock().await;
            // Passado o prazo, a próxima escrita tenta de novo (o lançador pode ter sido instalado).
            failures.retain(|_,(_,at)|at.elapsed() < LEGACY_WINDOW);
            failures.get(name).is_some_and(|(count,_)|*count >= LEGACY_TRIES)
        };
        if given_up {
            return Some(Err(RuntimeError::new("codex_terminal_relaunch_gave_up","o terminal desta sessão não reabriu depois de várias tentativas; feche e abra a sessão de novo")));
        }
        match self.cfg.mux.list_panes().await {
            Err(error) => return Some(Err(RuntimeError::new("codex_terminal_mux",&format!("o multiplexador não respondeu ({})",error.code)))),
            Ok(panes) if terminal::pane_of(&panes,name).is_some() => return Some(Err(RuntimeError::new("codex_terminal_without_control",
                "o terminal do Codex está aberto sem um app-server que o Hangar alcance; feche e abra a sessão de novo"))),
            Ok(_) => {}
        }
        // Só a falha do lançador ou da adoção conta para o teto: porta ocupada, multiplexador calado e
        // sidecar ilegível passam sem gastar tentativa.
        let mut launched = false;
        let result = async {
            let _closed = hold(registry.ingress(),name).await?;
            let _round = self.misses.lock().await;
            let meta = self.load(name).await.ok_or_else(gone)?;
            let key = terminal::text(&meta,"key").filter(|key|terminal::valid_key(key)).map_or_else(||random_hex(16),str::to_owned);
            let wanted = cleared(terminal_settings(meta,&json!({}),&key)?,false);
            launched = true;
            let pane = self.terminal_pane(registry,name,&key,&wanted).await?;
            self.open_terminal(registry,name,&key,&wanted,None,&pane).await
        }.await;
        let mut failures = self.legacy_failures.lock().await;
        match &result {
            Ok(()) => { failures.remove(name); }
            Err(_) if launched => {
                let entry = failures.entry(name.to_owned()).or_insert((0,std::time::Instant::now()));
                *entry = (entry.0 + 1,std::time::Instant::now());
            }
            Err(_) => {}
        }
        Some(result)
    }

    /// Vida nova na fila (atual + 1), sem a vida do terminal anterior: o escritor único da geração.
    async fn advance(&self,key:&str,name:&str) -> Result<u64,RuntimeError> {
        let (state_path,lease_path) = terminal::queue_files(&self.cfg.queue_dir,key);
        let (queue_dir,key,name) = (self.cfg.queue_dir.clone(),key.to_owned(),name.to_owned());
        tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
            let _lease = terminal::lease_blocking(&lease_path)?;
            let mut store = Store::open(&state_path,&queue_dir,QueueState::new(&key,1,&name,Vec::new()))?;
            let current = store.state().generation;
            let state = json!({"name":name,"codex_life":null});
            let id = terminal::life_call_id(store.state(),current + 1,&state);
            store.exec(current,&id,clock(),Action::AdvanceLifecycle { generation:current + 1,state })?;
            Ok(current + 1)
        }).await.map_err(|_|job())?.map_err(terminal::io_failure)
    }

    /// `POST /modo-execucao`. `None`: não é Codex do Rust, e o Python troca como hoje.
    pub async fn switch_mode(&self,registry:&RuntimeRegistry,name:&str,to_terminal:bool,read:PermissionRead) -> Option<Result<Value,RuntimeError>> {
        // Sem sidecar ainda, o pane Codex é a TUI abrindo a conversa (o lançador grava depois): é do Rust.
        let Some(meta) = self.load(name).await else {
            let opening = self.cfg.mux.list_panes().await.ok().is_some_and(|panes|codex_opening(&panes,name));
            return opening.then(||Err(starting()));
        };
        let headless = meta.get("headless").is_some_and(truthy);
        let adopted = registry.codex_terminals().await.iter().any(|t|t.name == name);
        if headless && let Err(error) = self.open_parked(registry,name).await? { return Some(Err(error)); }
        if headless != to_terminal {
            // Já no modo pedido: responde aqui só pela sessão aberta aqui.
            let ours = if headless { registry.codex_headless(name).await.is_some() } else { adopted };
            return ours.then(||Ok(json!({"ok":true,"terminal":to_terminal})));
        }
        if headless {
            registry.codex_headless(name).await?;
        } else if !adopted {
            match self.adopt_now(registry,name).await {
                Some(Ok(())) => {}
                Some(Err(error)) => return Some(Err(error)),
                // `launching` recente: a TUI ainda abre a conversa.
                None => return Some(Err(starting())),
            }
        }
        if self.transfer_busy(name,&meta).await { return Some(Err(busy())); }
        let switched = async {
            let _closed = hold(registry.ingress(),name).await?;
            let _round = self.misses.lock().await;
            // De novo sob a porta: uma transferência que começou no meio da espera também exclui a troca.
            let meta = self.load(name).await.ok_or_else(gone)?;
            if self.transfer_busy(name,&meta).await { return Err(busy()); }
            if to_terminal { self.to_terminal(registry,name).await } else { self.to_headless(registry,name,read).await }
        }.await;
        if switched.is_ok() { self.remember_switch(name).await; }
        Some(switched.map(|()|json!({"ok":true,"terminal":to_terminal})))
    }

    /// O spec gravado acompanha a troca (passo 4 da `SessionChange`): a superfície e o que a sessão tem
    /// agora no sidecar (o modo que o lançador gravou vence). Falha não desfaz a troca, que já
    /// valeu; aparece no log com o código.
    async fn remember_switch(&self,name:&str) {
        use crate::session_open::{ChangeReason,SessionChange,record};
        let (home,owned) = (self.cfg.home.clone(),name.to_owned());
        let written = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let Some(now) = record::from_sidecars(&home,&owned)? else { return Ok(()) };
            let spec = record::read(&home,&now.key)?.unwrap_or_else(||now.clone());
            let change = SessionChange { reason:ChangeReason::ModeSwitch,surface:Some(now.surface),account:None,permission_mode:now.permission_mode,
                model:now.model.clone(),effort:now.effort.clone(),service_tier:now.service_tier };
            record::write(&home,&spec.with(&change))
        }).await;
        if !matches!(written,Ok(Ok(()))) {
            tracing::warn!(session=%name,code="session_spec_unwritten","o registro da sessão não acompanhou a troca de modo");
        }
    }

    /// As recusas do Python antes de mexer em processo, com a frase dele.
    async fn preconditions(&self,registry:&RuntimeRegistry,handle:&RuntimeHandle,meta:&Meta,to_terminal:bool) -> Result<Value,RuntimeError> {
        let idle = if to_terminal { "Espere a sessão ficar ociosa antes de abrir o terminal." } else { "Espere a sessão ficar ociosa antes de continuar sem terminal." };
        let view = handle.view().await.map_err(|_|refuse(idle))?;
        let some = |v:&Value|v.as_array().is_some_and(|a|!a.is_empty()) || v.as_object().is_some_and(|o|!o.is_empty());
        // O motor do terminal fala por WebSocket (`connected`); o do sem terminal, pelo cano (`ready`).
        let engine_up = if to_terminal { view["ready"] == true } else { view["connected"] == true };
        if view["alive"] != true || !engine_up || view["in_progress"] == true || view["deliverable"] == false { return Err(refuse(idle)); }
        if some(&view["pending"]) || some(&view["async_questions"]) {
            return Err(refuse(if to_terminal { "Responda às perguntas e permissões antes de abrir o terminal." } else { "Responda às perguntas e permissões antes de trocar de modo." }));
        }
        let rows = handle.queue(format!("relaunch-load:{}",random_hex(8)),Action::Load).await?;
        if rows.as_array().is_some_and(|rows|rows.iter().any(|row|row["delivered"] == false)) { return Err(refuse(MSG_QUEUE)); }
        let first = if to_terminal { "Envie a primeira mensagem antes de abrir esta conversa no terminal." } else { "Envie a primeira mensagem antes de continuar sem terminal." };
        let (meta,base) = (meta.clone(),registry.codex_environment());
        tokio::task::spawn_blocking(move || {
            // Caminho relativo do sidecar seria resolvido contra a pasta do servidor, não a da sessão: recusa.
            let absolute = |field:&str|terminal::text(&meta,field).map(Path::new).filter(|path|path.is_absolute());
            if !absolute("rollout_path").is_some_and(Path::is_file) { return Err(refuse(first)); }
            if absolute("cwd").is_none() { return Err(refuse("A pasta da sessão não é um caminho absoluto; a sessão foi mantida.")); }
            let id = terminal::text(&meta,"codex_account").unwrap_or("default");
            let account = AccountService::new(base).resolve(Provider::Codex,id).map_err(|_|refuse(MSG_ACCOUNT))?;
            if absolute("codex_home").map(resolved) != Some(resolved(&account.home)) { return Err(refuse(MSG_ACCOUNT)); }
            Ok(())
        }).await.map_err(|_|job())??;
        Ok(view)
    }

    /// O pane do terminal pelo lançador único, o mesmo da criação: a receita sai do sidecar desta vida
    /// (modo gravado, nunca Full Access calado) e o só leitura do registro volta com ela. Montado
    /// antes de mexer em processo: lançador ausente ou envelope que não sobe recusam com tudo intacto.
    async fn terminal_pane(&self,registry:&RuntimeRegistry,name:&str,key:&str,meta:&Meta) -> Result<PaneSpec,RuntimeError> {
        use crate::session_open::codex;
        let base = registry.codex_environment();
        let refused = |error:crate::session_open::OpenError|refuse(&error.msg);
        // `_exigir_lancador_codex`: o pane morreria na hora com o `new-session` devolvendo sucesso.
        if super::process::resolve_program(codex::LAUNCHER,&base.base).is_none() {
            return Err(RuntimeError::new("codex_launcher_missing","hangar-codex-tui não está no PATH deste servidor; instale o lançador"));
        }
        if super::process::resolve_program("codex",&base.base).is_none() {
            return Err(RuntimeError::new("codex_cli_missing","codex não está no PATH deste servidor"));
        }
        let text = |field:&str|terminal::text(meta,field).map(str::to_owned);
        let cwd = text("cwd").ok_or_else(||refuse("A pasta da sessão não é um caminho absoluto; a sessão foi mantida."))?;
        let account = text("codex_account").unwrap_or_else(||"default".into());
        let codex_home = match text("codex_home") { Some(home)=>home, None=>codex::home(&base.base,&account).map_err(refused)? };
        let launch = codex::Launch { cwd:cwd.clone(),codex_home,account,service_tier:text("service_tier"),resume:text("thread_id"),
            model:text("model"),effort:text("effort"),mode:super::codex_launch::stored_mode(&Value::Object(meta.clone()))?.into(),
            tool_output_token_limit:meta.get("tool_output_token_limit").and_then(Value::as_u64),prompt:None };
        let read_only = if self.read_only(key).await? {
            let run = |argv:Vec<String>,timeout:Duration|->futures_util::future::BoxFuture<'_,crate::session_open::terminal::MuxOutput> {
                Box::pin(crate::session_open::server::run_program(&base.base,argv,timeout)) };
            Some(crate::session_open::open::read_only_envelope(&base.base,&base.home,&cwd,Some(&launch.codex_home),&run).await.map_err(refused)?)
        } else { None };
        let config = super::codex_launch::runtime_config(&base);
        codex::pane(name,key,&launch,meta.get("jev").is_some_and(truthy),&config,read_only.as_deref(),cfg!(windows)).map_err(refused)
    }

    /// Só leitura pelo registro da sessão; ilegível não prova que não é, e recusa.
    async fn read_only(&self,key:&str) -> Result<bool,RuntimeError> {
        let (home,key) = (self.cfg.home.clone(),key.to_owned());
        match tokio::task::spawn_blocking(move||crate::session_open::record::read(&home,&key)).await.map_err(|_|job())? {
            Ok(spec)=>Ok(spec.is_some_and(|spec|spec.read_only)),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput=>Ok(false),
            Err(_)=>Err(refuse("Não consegui ler o registro da sessão; a sessão foi mantida.")),
        }
    }

    /// Terminal → sem terminal (desenho 1.1).
    async fn to_headless(&self,registry:&RuntimeRegistry,name:&str,read:PermissionRead) -> Result<(),RuntimeError> {
        let found = registry.writable(name).await.ok_or_else(gone)?;
        let EntryHandle::CodexTerminal { target,handle,cell } = found.handle else { return Err(gone()) };
        let meta = self.load(name).await.filter(|meta|terminal::text(meta,"key") == Some(target.key.as_str())).ok_or_else(gone)?;
        let mut view = self.preconditions(registry,&handle,&meta,false).await?;
        // Sem terminal não há o envelope do só leitura; a sessão protegida fica no terminal.
        let marked = match terminal::pid(&meta,"app_pid").map(i64::from) {
            Some(pid)=>tokio::task::spawn_blocking(move||SystemProcs::default().env_var(pid,"HANGAR_ORQ_READ_ONLY")).await.map_err(|_|job())?
                .map_err(|_|refuse("Não consegui ler o ambiente do terminal (só leitura); a sessão foi mantida."))?.is_some_and(|v|v == "1"),
            None=>false,
        };
        if marked || self.read_only(&target.key).await? {
            return Err(refuse("Sessão só leitura não continua sem terminal: a proteção do código só existe no terminal. A sessão foi mantida."));
        }
        // O `/model` digitado na TUI pode não ter avisado o motor: a configuração sai da conversa, como o
        // `thread/resume` do Python. Sem ela a troca não sai: subiria com modelo, esforço ou modo velhos.
        let settings_read = handle.command(RuntimeCommand { operation_id:random_hex(16),kind:OperationKind::ReadSettings,payload:json!({"include_turns":false}) });
        match tokio::time::timeout(SETTINGS_READ_WAIT,settings_read).await {
            Ok(Ok(reply)) if reply.disposition == Disposition::Accepted => {
                for field in ["model","effort","service_tier","mode"] { if !reply.payload[field].is_null() { view[field] = reply.payload[field].clone(); } }
            }
            other => {
                let why = match other { Err(_) => "sem resposta no prazo".to_owned(), Ok(Err(error)) => error.code, Ok(Ok(reply)) => format!("{:?}",reply.disposition).to_lowercase() };
                tracing::warn!(session=%name,code="codex_settings_unread",reason=%why,"configuração da conversa não relida; troca recusada");
                return Err(refuse(&format!("Não consegui reler a configuração da conversa ({why}); a sessão foi mantida.")));
            }
        }
        // Retenção dos turnos antes do seletor e até o ator sair: nenhum `turn/start` entre a leitura e o kill.
        let turns = match handle.hold_turns().await {
            Ok(hold)=>hold,
            Err(error) if error.code == TURN_HOLD_BUSY=>return Err(refuse("Espere a sessão ficar ociosa antes de continuar sem terminal.")),
            Err(_)=>return Err(refuse(MSG_UNSUPPORTED)),
        };
        let current = match read {
            PermissionRead::Known(mode)=>Some(mode),
            PermissionRead::Pane(panels)=>{
                let driver = terminal::pane_driver(&target.terminal,cell.pane_serial(),panels);
                match crate::permission_control::codex(&driver,PermissionAction::Read { probe:false }).await {
                    Ok(read)=>read["current"].as_str().map(str::to_owned),
                    Err(error)=>{
                        tracing::warn!(session=%name,code=%error.code,"seletor de permissões não leu o modo; troca recusada");
                        // O motivo real (painel aberto, rascunho na TUI, menu aberto), não "modo não suportado".
                        return Err(refuse(&format!("Não consegui ler a permissão atual no terminal: {}. A sessão foi mantida.",error.message.trim_end_matches('.'))));
                    }
                }
            }
        };
        let permission = supported(current.as_deref()).ok_or_else(||refuse(MSG_UNSUPPORTED))?;
        // O modo lido vale nas duas direções: a volta ao terminal também relança com ele, nunca com o do
        // sidecar, que não acompanha o `/permissions` digitado direto na TUI.
        let mut settings = with_settings(meta.clone(),&view,&target.key);
        settings.insert("permission_mode".into(),json!(permission));
        // O Jev está no ambiente do processo vivo: o sidecar do terminal não o grava (`_jev_do_processo`).
        if let Some(pid) = terminal::pid(&meta,"app_pid").map(i64::from) {
            let read = tokio::task::spawn_blocking(move||SystemProcs::default().env_var(pid,"HANGAR_JEV")).await.map_err(|_|job())?;
            match read {
                Ok(value) => { settings.insert("jev".into(),json!(value.is_some_and(|v|v == "on"))); }
                // Sem ler o ambiente não se sabe se o Jev estava ligado: a troca não o desliga calada.
                Err(error) => {
                    tracing::warn!(session=%name,code="codex_jev_unread",kind=?error.kind(),"ambiente do Codex com terminal ilegível; troca recusada");
                    return Err(refuse("Não consegui ler o ambiente do terminal (Jev); a sessão foi mantida."));
                }
            }
        }
        let back = cleared(settings.clone(),false);
        let headless = cleared(settings,true);
        let back_pane = self.terminal_pane(registry,name,&target.key,&back).await.map_err(|error|refuse(&error.message))?;
        let mode = collaboration(&view);
        self.confirm_life(&target).await.map_err(|error|refuse(&error.message))?;
        let pane_pid = self.cfg.mux.list_panes().await.ok().and_then(|panes|terminal::pane_of(&panes,name).and_then(|pane|pane.pid));
        let roots:Vec<i64> = [terminal::pid(&meta,"app_pid"),terminal::pid(&meta,"tui_pid"),terminal::pid(&meta,"launcher_pid"),pane_pid]
            .into_iter().flatten().map(i64::from).collect();
        let lives = births(roots.clone()).await.map_err(|()|unread())?;
        // `app_pid` nulo antes do kill: o `finally` do lançador não apaga o sidecar e a vigia não readota.
        let expect = vec![("endpoint",json!(target.endpoint)),("app_pid",json!(target.app_pid)),("key",json!(target.key))];
        if self.sidecar_update(name,expect,json!({"app_pid":null})).await?.is_none() {
            return Err(refuse("A sessão mudou durante a troca; nada foi encerrado."));
        }
        if let Err(error) = registry.close(&target.key,target.generation).await {
            self.unpark(name,&target.key,target.app_pid).await;
            return Err(error);
        }
        drop(turns);
        let mux = MuxProcess::new(&self.cfg.mux_program,TIMEOUT);
        let killed = match mux.kill_session(name).await {
            Some(true)=>None,
            Some(false)=>Some(RuntimeError::new("relaunch_kill_survived","Não foi possível fechar o terminal; o modo foi mantido.")),
            None=>Some(RuntimeError::new("relaunch_kill_mux","o multiplexador não respondeu; o modo foi mantido")),
        };
        if let Some(error) = killed {
            // O pane segue vivo: ou volta a ter dono aqui, ou a resposta diz que ficou sem dono.
            let orphan = if !self.unpark(name,&target.key,target.app_pid).await { Some("o sidecar mudou de vida".to_owned()) }
                else { match self.adopt_locked(registry,name).await {
                    Some(Ok(()))=>None,
                    Some(Err(adopt))=>Some(adopt.message),
                    None=>Some("o sidecar deixou de ser de terminal".to_owned()),
                } };
            let Some(reason) = orphan else { return Err(error) };
            tracing::warn!(session=%name,code=%error.code,%reason,"terminal que não fechou ficou sem dono");
            return Err(RuntimeError::new(&error.code,&format!("{} O terminal continua aberto, mas sem dono aqui: {reason}.",error.message)));
        }
        if let Err(left) = ensure_absent(lives,&roots).await {
            // Sem `unpark`: com o pane morto, `app_pid` de volta faria a vigia enterrar a sessão (sidecar e
            // fila). Nulo, ela fica de fora e a sessão segue visível com este erro, sem readoção.
            return Err(alive_error("O terminal ainda está encerrando; não abri outro processo. A sessão ficou parada sem terminal: \
                feche-a e abra a conversa de novo quando esses processos saírem.",&left));
        }
        let Err(error) = self.open_headless(registry,name,&target.key,&headless,mode.as_deref()).await else { return Ok(()) };
        // O sem terminal que falhou não saiu: outro terminal não sobe ao lado dele.
        if kill_phase(&error) { return Err(error); }
        tracing::warn!(session=%name,code=%error.code,"troca para sem terminal falhou; voltando ao terminal");
        match self.open_terminal(registry,name,&target.key,&back,mode.as_deref(),&back_pane).await {
            Ok(())=>Err(refuse(&format!("A troca falhou; a conversa continua no terminal: {}",error.message))),
            Err(restore)=>Err(restored_failure(&format!("A troca falhou ({}) e o terminal não voltou: {}",error.message,restore.message),&restore)),
        }
    }

    /// Sem terminal → terminal (desenho 1.2).
    async fn to_terminal(&self,registry:&RuntimeRegistry,name:&str) -> Result<(),RuntimeError> {
        let (key,generation,handle) = registry.codex_headless(name).await.ok_or_else(gone)?;
        let meta = self.load(name).await.filter(|meta|terminal::text(meta,"key") == Some(key.as_str())).ok_or_else(gone)?;
        let view = self.preconditions(registry,&handle,&meta,true).await?;
        let settings = terminal_settings(meta.clone(),&view,&key)?;
        let back = cleared(settings.clone(),true);
        let wanted = cleared(settings,false);
        let pane = self.terminal_pane(registry,name,&key,&wanted).await.map_err(|error|refuse(&error.message))?;
        match MuxProcess::new(&self.cfg.mux_program,TIMEOUT).has_session(name).await {
            Some(false)=>{},
            Some(true)=>return Err(refuse("Já existe um terminal com este nome.")),
            None=>return Err(refuse("O multiplexador não respondeu; a sessão foi mantida.")),
        }
        let turns = match handle.hold_turns().await {
            Ok(hold)=>hold,
            Err(_)=>return Err(refuse("Espere a sessão ficar ociosa antes de abrir o terminal.")),
        };
        let mode = collaboration(&view);
        let cano = meta.get("cano").and_then(|cano|cano.get("pid")).and_then(Value::as_i64);
        let roots:Vec<i64> = cano.into_iter().collect();
        let lives = births(roots.clone()).await.map_err(|()|unread())?;
        registry.close_with_kill(&key,generation).await?;
        drop(turns);
        if let Err(left) = ensure_absent(lives,&roots).await {
            return Err(alive_error("O processo antigo ainda está vivo; nenhum terminal foi aberto. A sessão ficou parada: \
                feche-a e abra a conversa de novo quando esses processos saírem.",&left));
        }
        let Err(error) = self.open_terminal(registry,name,&key,&wanted,mode.as_deref(),&pane).await else { return Ok(()) };
        // Pane que não fechou: outro processo não sobe ao lado dele.
        if kill_phase(&error) { return Err(error); }
        tracing::warn!(session=%name,code=%error.code,"troca para terminal falhou; voltando ao sem terminal");
        match self.open_headless(registry,name,&key,&back,mode.as_deref()).await {
            Ok(())=>Err(refuse(&format!("A troca falhou; a conversa continua sem terminal: {}",error.message))),
            Err(restore)=>Err(restored_failure(&format!("A troca falhou ({}) e a sessão não reiniciou: {}",error.message,restore.message),&restore)),
        }
    }

    /// Grava o sidecar sem terminal, abre a vida nova na fila e sobe o cano (`open_with_launch`, políticas
    /// locais da U1). Falhou: o que subiu morre aqui.
    async fn open_headless(&self,registry:&RuntimeRegistry,name:&str,key:&str,meta:&Meta,mode:Option<&str>) -> Result<(),RuntimeError> {
        self.sidecar_put(name,meta).await?;
        let generation = self.advance(key,name).await?;
        let thread = terminal::text(meta,"thread_id").unwrap_or_default().to_owned();
        let target = headless_target(&self.cfg.queue_dir,name,key,generation,meta);
        let result = async {
            registry.open_with_launch(target,codex_sidecar::dir(&self.cfg.home)).await?;
            let handle = registry.handle(key,generation).await?;
            settled(&handle,"ready",&thread).await?;
            same_permission(&self.load(name).await.unwrap_or_default(),meta)?;
            if let Some(mode) = mode { set_mode(&handle,mode).await?; }
            Ok::<(),RuntimeError>(())
        }.await;
        let Err(error) = result else { return Ok(()) };
        // A subida que falhou pode deixar o cano vivo (o `open` que fecha o ator sem matar): o que ela gravou
        // no sidecar morre aqui, e a saída é conferida antes de qualquer volta subir outro processo ao lado.
        let cano = self.load(name).await.and_then(|meta|meta.get("cano").cloned())
            .and_then(|cano|serde_json::from_value::<super::process::Cano>(cano).ok());
        let roots:Vec<i64> = cano.iter().map(|cano|i64::from(cano.pid)).collect();
        let Ok(lives) = births(roots.clone()).await else {
            return Err(RuntimeError::new("relaunch_kill_read",&format!("{} Não consegui conferir se o Codex sem terminal desta tentativa saiu; não abri outro processo.",error.message)));
        };
        let killed = match registry.close_with_kill(key,generation).await {
            Ok(reply)=>reply["killed"] == true,
            Err(stop)=>{ tracing::warn!(session=%name,code=%stop.code,"sem terminal que falhou na troca não fechou"); false },
        };
        if !killed && let Some(cano) = &cano && let Err(stop) = super::process::kill(cano,key,&codex_sidecar::dir(&self.cfg.home)).await {
            tracing::warn!(session=%name,code=stop.code(),"cano do sem terminal que falhou na troca não encerrou");
        }
        if let Err(left) = ensure_absent(lives,&roots).await {
            return Err(alive_error(&format!("{} O Codex sem terminal desta tentativa ainda está vivo; não abri outro processo.",error.message),&left));
        }
        Err(error)
    }

    /// Grava o sidecar de terminal, cria o pane pelo lançador único (`session_open::terminal`, o mesmo da
    /// criação, com o ambiente da sessão e as variáveis de tela), espera a TUI, adota e restaura o modo
    /// depois da assinatura. Falhou: pane, ator e processos desta tentativa saem.
    async fn open_terminal(&self,registry:&RuntimeRegistry,name:&str,key:&str,meta:&Meta,mode:Option<&str>,pane:&PaneSpec) -> Result<(),RuntimeError> {
        self.sidecar_put(name,meta).await?;
        self.advance(key,name).await?;
        let thread = terminal::text(meta,"thread_id").unwrap_or_default().to_owned();
        let mux = MuxProcess::new(&self.cfg.mux_program,TIMEOUT);
        let result = async {
            let (base,program) = (registry.codex_environment(),self.cfg.mux_program.to_string_lossy().into_owned());
            let env = tokio::task::spawn_blocking(move||crate::session_open::terminal::TerminalEnv {
                program,..crate::session_open::terminal::TerminalEnv::local(&base.home,&base.base) }).await.map_err(|_|job())?;
            crate::session_open::terminal::spawn(&crate::session_open::terminal::ProcessMux::default(),&env,pane).await
                .map_err(|error|RuntimeError::new(error.code,&format!("o terminal não abriu: {}",error.stderr)))?;
            let up = super::codex_pane::wait_terminal(&mux,&codex_sidecar::dir(&self.cfg.home),name,key,&thread,super::codex_pane::TERMINAL_WAIT).await?;
            same_permission(&up,meta)?;
            match self.adopt_locked(registry,name).await {
                Some(Ok(()))=>{},
                Some(Err(error))=>return Err(error),
                None=>return Err(RuntimeError::new("codex_terminal_not_ready","o terminal não ficou pronto para adotar")),
            }
            let adopted = registry.codex_terminals().await.into_iter().find(|t|t.name == name)
                .ok_or_else(||RuntimeError::new("codex_terminal_unadopted","o terminal foi adotado e fechou em seguida"))?;
            let handle = registry.handle(&adopted.key,adopted.generation).await?;
            // `wait_terminal` não confere a thread carregada: o modo só vale depois da assinatura.
            settled(&handle,"subscribed",&thread).await?;
            if let Some(mode) = mode { set_mode(&handle,mode).await?; }
            Ok::<(),RuntimeError>(())
        }.await;
        let Err(error) = result else { return Ok(()) };
        if let Err(dropped) = self.drop_terminal(registry,name,&mux).await {
            // As duas falhas aparecem: a da tentativa e a de não ter conseguido desfazê-la.
            tracing::warn!(session=%name,code=%error.code,drop=%dropped.code,"tentativa de terminal falhou e não foi desfeita");
            return Err(RuntimeError::new(&dropped.code,&format!("{} (a tentativa falhou antes: {})",dropped.message,error.message)));
        }
        Err(error)
    }

    /// Desfaz uma tentativa de terminal: ator adotado, pane pelo nome (conferido) e os processos dela.
    async fn drop_terminal(&self,registry:&RuntimeRegistry,name:&str,mux:&MuxProcess) -> Result<(),RuntimeError> {
        if let Some(adopted) = registry.codex_terminals().await.into_iter().find(|t|t.name == name)
            && let Err(error) = registry.close(&adopted.key,adopted.generation).await {
            tracing::warn!(session=%name,code=%error.code,"ator do terminal que falhou na troca não fechou");
        }
        let meta = self.load(name).await.unwrap_or_default();
        let pane_pid = self.cfg.mux.list_panes().await.ok().and_then(|panes|terminal::pane_of(&panes,name).and_then(|pane|pane.pid));
        let roots:Vec<i64> = [terminal::pid(&meta,"app_pid"),terminal::pid(&meta,"tui_pid"),terminal::pid(&meta,"launcher_pid"),pane_pid]
            .into_iter().flatten().map(i64::from).collect();
        let lives = births(roots.clone()).await
            .map_err(|()|RuntimeError::new("relaunch_kill_read","Não consegui ler os processos do terminal desta tentativa; não abri outro processo."))?;
        // Pane que não fecha pode terminar de subir depois: a vigia o adota, e a resposta tem de dizer isso.
        let still_open = "Ele continua aberto: se terminar de subir, a conversa segue nele. Não abri outro processo.";
        match mux.kill_session(name).await {
            Some(true)=>{},
            Some(false)=>return Err(RuntimeError::new("relaunch_kill_survived",&format!("Não consegui fechar o terminal desta tentativa. {still_open}"))),
            None=>return Err(RuntimeError::new("relaunch_kill_mux",&format!("O multiplexador não respondeu ao fechar o terminal desta tentativa. {still_open}"))),
        }
        if let Err(left) = ensure_absent(lives,&roots).await {
            return Err(alive_error("O terminal desta tentativa ainda está encerrando; não abri outro processo.",&left));
        }
        Ok(())
    }

    /// `POST /recarregar` do Codex do Rust (desenho 1.4). `None`: não é Codex do Rust, ou há transferência
    /// sem fase final (o Python a recupera como hoje).
    pub async fn restart(&self,registry:&RuntimeRegistry,name:&str) -> Option<Result<Value,RuntimeError>> {
        let meta = self.load(name).await?;
        if self.transfer_busy(name,&meta).await { return None; }
        if !meta.get("headless").is_some_and(truthy) {
            // Só leitura: perguntar se é do Rust não adota nem relança o terminal (o sidecar antigo relançaria).
            use crate::session_write::{Owner,Provider as Route,WriteRoute,decide};
            let ours = registry.codex_terminals().await.iter().any(|t|t.name == name) || decide(WriteRoute::CodexControl,Route::Codex,true) == Owner::Rust;
            return ours.then(||Err(RuntimeError::new("erro_recarregar_so_sem_terminal","reiniciar só vale para sessão Codex sem terminal")));
        }
        if let Err(error) = self.open_parked(registry,name).await? { return Some(Err(error)); }
        let (_,_,handle) = registry.codex_headless(name).await?;
        let _closed = match hold(registry.ingress(),name).await { Ok(closed)=>closed, Err(error)=>return Some(Err(error)) };
        // Vale com a sessão trabalhando: é a saída de um turno que nunca fecha.
        let reply = handle.command(RuntimeCommand { operation_id:random_hex(16),kind:OperationKind::Restart,payload:json!({}) }).await;
        Some(match reply {
            Ok(reply) if reply.disposition == Disposition::Accepted=>Ok(json!({"ok":true})),
            Ok(reply)=>Err(RuntimeError::new("erro_codex_nao_reiniciou",reply.payload["error"].as_str().unwrap_or("o runtime recusou o reinício"))),
            // O código do runtime segue: `reload_refusal` decide o status e o leva em `params.code`.
            Err(error)=>Err(error),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_codex_pane_without_sidecar_is_still_opening() {
        let pane = |provider:Option<&str>| crate::list::mux::Pane { session:"s".into(), active:true, pid:Some(1), cwd:"/w".into(),
            pane_id:"%1".into(), hidden:false, provider:provider.map(str::to_owned), session_created:Some(1), window_index:Some(0), pane_index:Some(0), dead:false };
        assert!(codex_opening(&[pane(Some("codex"))],"s"));
        for other in [Some("claude"),Some("pi"),None] {
            assert!(!codex_opening(&[pane(other)],"s"),"{other:?}: não é a TUI do Codex abrindo");
        }
        assert!(!codex_opening(&[pane(Some("codex"))],"outra"));
    }

    #[test]
    fn only_the_three_known_modes_go_headless() {
        assert_eq!(supported(Some("Ask for approval")),Some("Ask for approval"));
        assert_eq!(supported(Some(" approve for me ")),Some("Approve for me"));
        assert_eq!(supported(Some("Full Access")),Some("Full Access"));
        for unknown in [None,Some(""),Some("Read Only"),Some("Custom (config.toml)")] {
            assert_eq!(supported(unknown),None,"{unknown:?}: desconhecido recusa, nunca Full Access");
        }
    }

    #[test]
    fn an_open_transfer_for_the_name_blocks_the_switch() {
        let home = tempfile::tempdir().unwrap();
        let base = home.path().join(".hangar").join("conversation-transfers");
        std::fs::create_dir_all(base.join("indexes")).unwrap();
        assert!(!transfer_open(home.path(),"cx",None,None),"sem registro");
        let id = "0000000a-0000-0000-0000-000000000001";
        std::fs::write(base.join(format!("{id}.json")),json!({"name":"cx","phase":"publishing"}).to_string()).unwrap();
        assert!(transfer_open(home.path(),"cx",Some(id),None),"pelo sidecar");
        let hex:String = ring::digest::digest(&ring::digest::SHA256,b"cx").as_ref().iter().map(|b|format!("{b:02x}")).collect();
        std::fs::write(base.join("indexes").join(format!("session-{hex}.json")),json!([id]).to_string()).unwrap();
        assert!(transfer_open(home.path(),"cx",None,None),"pelo índice da sessão");
        // Registro aberto de outra vida do mesmo nome não segura esta; o desta vida, sim.
        std::fs::write(base.join(format!("{id}.json")),json!({"name":"cx","phase":"publishing","source_life":"k:old"}).to_string()).unwrap();
        assert!(!transfer_open(home.path(),"cx",None,Some("new")),"outra vida");
        assert!(transfer_open(home.path(),"cx",None,Some("old")),"esta vida");
        assert!(transfer_open(home.path(),"cx",Some(id),Some("new")),"marcado no sidecar vale sempre");
        assert!(!transfer_open(home.path(),"other",None,None));
        std::fs::write(base.join(format!("{id}.json")),json!({"name":"cx","phase":"complete"}).to_string()).unwrap();
        assert!(!transfer_open(home.path(),"cx",Some(id),None),"fase final libera");
        assert!(transfer_open(home.path(),"cx",Some("../x"),None),"id torto não sai da pasta e conta como em curso");
    }

    #[test]
    fn the_headless_sidecar_takes_the_engine_settings_and_drops_the_terminal_process() {
        let meta:Meta = json!({"name":"cx","key":"old","model":"m0","endpoint":"ws://127.0.0.1:1","app_pid":7,"tui_pid":8,"launcher_pid":9,"cano":{"pid":1}})
            .as_object().unwrap().clone();
        let settings = with_settings(meta,&json!({"model":"m1","effort":"high","service_tier":null}),"k1");
        let headless = cleared(settings.clone(),true);
        assert_eq!((headless["model"].as_str(),headless["effort"].as_str(),headless["key"].as_str()),(Some("m1"),Some("high"),Some("k1")));
        for field in ["endpoint","app_pid","tui_pid","launcher_pid","cano"] { assert!(headless[field].is_null(),"{field}"); }
        assert_eq!(headless["headless"],true);
        assert_eq!(cleared(settings,false)["headless"],false);
        assert_eq!(collaboration(&json!({"mode":"plan"})).as_deref(),Some("plan"));
        assert_eq!(collaboration(&json!({"mode":"other"})),None);
    }

    #[test]
    fn the_terminal_sidecar_records_the_opening_default_and_refuses_an_unknown_mode() {
        let map = |v:Value|v.as_object().unwrap().clone();
        let null = terminal_settings(map(json!({"name":"cx","permission_mode":null})),&json!({}),"k").unwrap();
        assert_eq!(null["permission_mode"],"Full Access","nulo é o padrão da abertura, gravado para o lançador");
        let kept = terminal_settings(map(json!({"name":"cx","permission_mode":"ask for approval"})),&json!({}),"k").unwrap();
        assert_eq!(kept["permission_mode"],"Ask for approval");
        let unknown = terminal_settings(map(json!({"name":"cx","permission_mode":"Read Only"})),&json!({}),"k").unwrap_err();
        assert_eq!(unknown.code,"launch_permission_mode","desconhecido nunca vira Full Access");
    }

    #[test]
    fn a_failed_way_back_keeps_the_500_only_when_a_process_did_not_exit() {
        let alive = restored_failure("A troca falhou (x) e o terminal não voltou: y",&RuntimeError::new("relaunch_kill_survived","y"));
        assert_eq!(alive.code,"relaunch_kill_survived");
        assert!(kill_phase(&alive),"processo vivo ao lado: 500, nunca uma recusa comum");
        let refused = restored_failure("m",&RuntimeError::new("codex_terminal_timeout","y"));
        assert_eq!((refused.code.as_str(),kill_phase(&refused)),("erro_troca_modo",false));
        assert!(kill_phase(&RuntimeError::new("relaunch_kill_read","x")),"leitura que não prova a saída também segura a volta");
    }

    #[test]
    fn the_new_process_must_read_the_permission_mode_the_switch_wrote() {
        let map = |v:Value|v.as_object().unwrap().clone();
        let wanted = map(json!({"permission_mode":"Ask for approval"}));
        assert!(same_permission(&map(json!({"permission_mode":"Ask for approval"})),&wanted).is_ok());
        for read in [json!({"permission_mode":"Full Access"}),json!({})] {
            assert_eq!(same_permission(&map(read),&wanted).unwrap_err().code,"relaunch_permission");
        }
        assert!(same_permission(&map(json!({})),&map(json!({}))).is_err(),"modo desconhecido nos dois lados não passa");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn births_lists_a_live_process_with_its_descendants_and_skips_the_exited() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut parent = std::process::Command::new("sh").args(["-c","sleep 60 & wait"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let root = i64::from(parent.id());
        let lives = rt.block_on(births(vec![root,0,1])).unwrap();
        assert!(lives.iter().any(|(pid,_)|*pid == root));
        assert!(lives.len() >= 2,"o filho `sleep` entra na espera: {lives:?}");
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let gone_pid = i64::from(gone.id());
        gone.wait().unwrap();
        assert!(rt.block_on(births(vec![gone_pid])).unwrap().is_empty(),"processo que saiu não entra");
        // Só a raiz leva o sinal: o descendente (um servidor deixado de pé de propósito) é esperado, nunca
        // encerrado, e a recusa diz qual pid segura a troca.
        let left = rt.block_on(ensure_absent(lives.clone(),&[root])).unwrap_err();
        let child:Vec<i64> = lives.iter().map(|(pid,_)|*pid).filter(|pid|*pid != root).collect();
        assert!(!left.contains(&root),"a raiz recebeu o SIGTERM e saiu");
        assert_eq!(left,child,"o filho segue vivo: não foi sinalizado");
        assert!(alive_error("x",&left).message.contains(&child[0].to_string()));
        for pid in child { let _ = std::process::Command::new("kill").arg(pid.to_string()).status(); }
        let _ = parent.wait();
    }
}
