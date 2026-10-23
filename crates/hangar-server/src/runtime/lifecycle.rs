//! Ciclo de vida Claude, sob a barreira e a lease já usadas pelo registro.
use crate::{interactions::InteractionBinding,list::procs::{ProcessView,ProcessRecipe},terminal_input::{TerminalBinding,TerminalIo,CommandRequest}};
use super::protocol::RuntimeError;
use std::time::Duration;
impl crate::state::live::Lifecycle for super::gateway::RuntimeRegistry {
    fn dead<'a>(&'a self,name:&'a str,key:&'a str,conversation:&'a str,generation:u64)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<crate::state::facts::Dead,String>>+Send+'a>> {
        Box::pin(async move {super::gateway::RuntimeRegistry::dead(self,name,key,conversation,generation).await.map_err(|error|error.code)})
    }
}

fn failure(code:&str)->RuntimeError {RuntimeError::new(code,"a vida da sessão não foi comprovada; nenhum processo alheio será encerrado")}

#[derive(Default)]
pub(super) struct Barrier {
    serial:tokio::sync::Mutex<()>,
    pub operation:std::sync::Mutex<Option<(String,std::sync::Arc<std::sync::atomic::AtomicBool>)>>,
    pub closed:std::sync::Mutex<Option<ClosedLife>>,
    /// Envios pela porta privada leem; estacionar só escreve com nenhum em curso.
    pub park:tokio::sync::RwLock<()>,
    /// Nome da sessão estacionada desta chave: o próximo envio a reabre pelo arquivo dela.
    pub parked:std::sync::Mutex<Option<String>>,
}
#[derive(Clone)]
pub(super) struct ClosedLife {
    pub name:String,pub binding:InteractionBinding,pub pid:i64,pub birth:f64,
    pub state_path:std::path::PathBuf,pub lease_path:std::path::PathBuf,pub projection:std::path::PathBuf,
}
impl std::ops::Deref for Barrier {type Target=tokio::sync::Mutex<()>;fn deref(&self)->&Self::Target {&self.serial}}
impl Barrier {pub fn cancel(&self) {if let Ok(op)=self.operation.lock() && let Some((_,cancel))=op.as_ref() {cancel.store(true,std::sync::atomic::Ordering::Release);}}}

pub(super) struct HeadlessOperation {
    pub target:super::protocol::RuntimeTarget,
    pub binding:InteractionBinding,
    pub recipe:std::sync::Arc<super::claude_relaunch::Recipe>,
    pub view:serde_json::Value,
    pub birth:f64,
    pub auto:Option<Duration>,
    pub cancelled:std::sync::Arc<std::sync::atomic::AtomicBool>,
}
pub struct ClosedIngress {gates:super::ingress::IngressGates,name:String}
impl ClosedIngress {
    pub async fn acquire(gates:&super::ingress::IngressGates,name:&str)->Result<Self,RuntimeError> {
        Self::acquire_within(gates,name,Duration::from_secs(60)).await
    }
    pub async fn acquire_within(gates:&super::ingress::IngressGates,name:&str,wait:Duration)->Result<Self,RuntimeError> {
        gates.close(name,wait).await.map_err(|_|failure("lifecycle_ingress_busy"))?;
        Ok(Self {gates:gates.clone(),name:name.into()})
    }
    /// A porta retida pela troca de conversa recusa na hora, com o código da transferência.
    pub async fn acquire_unless_transfer(gates:&super::ingress::IngressGates,name:&str)->Result<Self,RuntimeError> {
        gates.close_unless_held(name,Duration::from_secs(60)).await.map_err(|refused|match refused {
            super::ingress::CloseRefused::Held=>RuntimeError::new("session_transfer_busy","a sessão está trocando de agente"),
            super::ingress::CloseRefused::Busy=>failure("lifecycle_ingress_busy"),
        })?;
        Ok(Self {gates:gates.clone(),name:name.into()})
    }
}
impl Drop for ClosedIngress {fn drop(&mut self) {self.gates.open(&self.name);}}

pub(super) fn retry_fence(binding:&InteractionBinding,state:&super::queue::State,id:&str,attempt:u32,cancelled:bool,active:Option<&InteractionBinding>)->Result<(u64,u64),RuntimeError> {
    if cancelled {return Err(failure("lifecycle_cancelled"));}
    if active.is_some_and(|active|active!=binding) {return Err(failure("runtime_binding"));}
    let next=binding.generation.checked_add(1).ok_or_else(||failure("runtime_generation"))?;
    if state.owner_key!=binding.key || ![binding.generation,next].contains(&state.generation) {return Err(failure("runtime_generation"));}
    let intent=serde_json::json!({"operation_id":id,"key":binding.key,"conversation":binding.conversation,"from_generation":binding.generation,"target_generation":next});
    if attempt>0 && state.runtime_state["claude_lifecycle_intent"]!=intent {return Err(failure("lifecycle_intent_changed"));}
    if state.runtime_state.get("metadata").is_some_and(|meta|meta["key"]!=binding.key || meta["session_id"]!=binding.conversation) {return Err(failure("lifecycle_conversation"));}
    Ok((state.generation,next))
}

pub fn matches_life(binding:&InteractionBinding,key:&str,conversation:&str,generation:u64)->bool {
    binding.key==key && binding.conversation==conversation && binding.generation==generation
}
async fn mux(io:&dyn TerminalIo,binding:&TerminalBinding,args:Vec<String>)->Result<crate::terminal_input::CommandOutput,RuntimeError> {
    let (program,prefix)=binding.mux_argv.split_first().ok_or_else(||failure("lifecycle_mux"))?;
    let mut full=prefix.to_vec();full.extend(args);
    io.command(CommandRequest {program:program.clone(),args:full,stdin:vec![]}).await.map_err(|_|failure("lifecycle_mux"))
}
pub struct Pane {pub pid:i64,pub dead:bool,pub copy:bool,pub alternate:bool}
/// O alvo do Windows inclui sessão e índices, nunca o `%1` global do psmux.
pub async fn pane(io:&dyn TerminalIo,binding:&TerminalBinding)->Result<Pane,RuntimeError> {
    let out=mux(io,binding,vec!["display-message".into(),"-p".into(),"-t".into(),binding.pane.clone(),
        "#{session_name}\t#{pane_id}\t#{session_created}\t#{pane_pid}\t#{pane_dead}\t#{pane_in_mode}\t#{alternate_on}".into()]).await?;
    if !out.success {return Err(failure("lifecycle_capture"));}
    let raw=std::str::from_utf8(&out.stdout).map_err(|_|failure("lifecycle_capture"))?;
    if binding.windows && raw.contains('\u{fffd}') {return Err(failure("lifecycle_capture"));}
    let fields:Vec<_>=raw.trim_end().split('\t').collect();
    let pane_id=if binding.windows {"%1"} else {binding.pane.as_str()};
    if fields.len()!=7 || fields[0]!=binding.name || fields[1]!=pane_id || fields[2].parse::<u64>().ok()!=Some(binding.created) {return Err(failure("lifecycle_binding"));}
    let binary=|at:usize|match fields[at] {"0"=>Ok(false),"1"=>Ok(true),_=>Err(failure("lifecycle_capture"))};
    let pid=fields[3].parse::<i64>().ok().filter(|p|*p>0).ok_or_else(||failure("lifecycle_capture"))?;
    Ok(Pane {pid,dead:binary(4)?,copy:binary(5)?,alternate:binary(6)?})
}
pub fn process_absent(procs:&dyn ProcessView,pid:i64,birth:f64)->Result<bool,RuntimeError> {
    match procs.recipe(pid) {
        Ok(recipe)=>Ok(!crate::terminal_facts::same_birth(recipe.birth,birth)),
        Err(error) if crate::list::procs::exited(&error)=>Ok(true),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=> {
            let children=procs.children(Duration::ZERO).map_err(|_|failure("lifecycle_process_read"))?;
            Ok(!children.values().any(|pids|pids.contains(&pid)))
        },
        Err(_)=>Err(failure("lifecycle_process_read")),
    }
}
/// O agente desta vida morreu (pid com o mesmo nascimento ausente). Tabela de processos ilegível não prova
/// morte: conta como vivo e vai ao log com o código.
pub fn agent_gone(name:&str,pid:i64,birth:f64)->bool {
    super::lifecycle::process_absent(&crate::list::procs::SystemProcs::default(),pid,birth).unwrap_or_else(|error| {
        if crate::warn_limit::allow(Some(name),&error.code) {
            tracing::warn!(session=%name,code=%error.code,"processos ilegíveis: a vida guardada segue como viva");
        }
        false
    })
}
/// Não mata por pid solto: reconfere pane e identidade imediatamente antes da execução.
pub async fn kill_terminal(io:&dyn TerminalIo,procs:&dyn ProcessView,binding:&TerminalBinding,agent_pid:i64,birth:f64)->Result<(),RuntimeError> {
    let before=pane(io,binding).await?;
    if before.copy {return Err(failure("lifecycle_panel"));}
    let recipe=procs.recipe(agent_pid).map_err(|_|failure("lifecycle_process_read"))?;
    if !crate::terminal_facts::same_birth(recipe.birth,birth) {return Err(failure("lifecycle_process_changed"));}
    let children=procs.children(Duration::ZERO).map_err(|_|failure("lifecycle_process_read"))?;
    let mut pending=vec![before.pid];let mut found=false;let mut seen=std::collections::HashSet::new();
    while let Some(pid)=pending.pop() {
        if !seen.insert(pid) {continue;}
        if pid==agent_pid {found=true;break;}
        pending.extend(children.get(&pid).into_iter().flatten().copied());
    }
    if !found {return Err(failure("lifecycle_process_binding"));}
    let after=pane(io,binding).await?;
    if after.pid!=before.pid || procs.recipe(agent_pid).map_err(|_|failure("lifecycle_process_read"))?!=recipe {return Err(failure("lifecycle_process_changed"));}
    let target=if binding.windows {binding.name.clone()} else {format!("={}",binding.name)};
    let output=mux(io,binding,vec!["kill-session".into(),"-t".into(),target]).await?;
    if !output.success {return Err(failure("lifecycle_kill"));}
    wait_absent(procs,agent_pid,birth,"lifecycle_still_alive").await
}

/// Encerrar só avisa o processo (SIGHUP, SIGTERM): ele leva um instante para sair, e no meio da saída a leitura dele
/// pode falhar. Só o prazo decide: sai o último resultado, `alive` (vivo) ou o erro da leitura.
pub async fn wait_absent(procs:&dyn ProcessView,pid:i64,birth:f64,alive:&str)->Result<(),RuntimeError> {
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    loop {
        let last=match process_absent(procs,pid,birth) {
            Ok(true)=>return Ok(()),
            Ok(false)=>failure(alive),
            Err(error)=>error,
        };
        if std::time::Instant::now()>=deadline {return Err(last);}
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn kill_pane(io:&dyn TerminalIo,binding:&TerminalBinding,expected:&ProcessRecipe,procs:&dyn ProcessView)->Result<(),RuntimeError> {
    let current=pane(io,binding).await?;
    if current.dead || current.copy || procs.recipe(expected.pid).map_err(|_|failure("lifecycle_process_read"))?!=*expected {return Err(failure("lifecycle_process_changed"));}
    let output=mux(io,binding,vec!["kill-pane".into(),"-t".into(),binding.pane.clone()]).await?;
    if !output.success {return Err(failure("lifecycle_kill"));}
    wait_absent(procs,expected.pid,expected.birth,"lifecycle_kill").await
}

pub fn terminal_choices(target:&super::terminal::TerminalTarget)->Result<Option<crate::list::facts_files::Published>,RuntimeError> {
    let path=target.metadata.config_dir.join(".hangar-status").join(format!("{}.json",target.binding.conversation));
    let before=match std::fs::read(&path) {Ok(bytes)=>bytes,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(_)=>return Err(failure("lifecycle_choices_read"))};
    let value:serde_json::Value=serde_json::from_slice(&before).map_err(|_|failure("lifecycle_choices_read"))?;
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|failure("lifecycle_clock"))?.as_secs_f64();
    let ts=value["ts"].as_f64().filter(|ts|ts.is_finite()).ok_or_else(||failure("lifecycle_choices_time"))?;
    if ts<target.metadata.cut(target.created) || ts>now+5.0 {return Err(failure("lifecycle_choices_old_life"));}
    let meta=std::fs::metadata(&path).map_err(|_|failure("lifecycle_choices_read"))?;
    let published=crate::list::facts_files::published_status(Some(&target.binding.conversation),&[target.metadata.config_dir.clone()],now);
    let after=std::fs::read(&path).map_err(|_|failure("lifecycle_choices_read"))?;
    let latest=std::fs::metadata(&path).map_err(|_|failure("lifecycle_choices_read"))?;
    if before!=after || meta.modified().ok()!=latest.modified().ok() || meta.len()!=latest.len() {return Err(failure("lifecycle_choices_changed"));}
    Ok(published)
}

pub fn target_value(target:&super::terminal::TerminalTarget)->serde_json::Value {
    serde_json::json!({"key":target.key,"generation":target.generation,"name":target.name,"binding":target.binding,
        "lease_path":target.lease_path,"state_path":target.state_path,"projection_dir":target.projection_dir,"transcript":target.transcript,"created":target.created,
        "plugin_key":target.plugin_key,"metadata":{"config_dir":target.metadata.config_dir,"agent_pid":target.metadata.agent_pid,
            "agent_birth":target.metadata.agent_birth,"legacy_import_after":target.metadata.legacy_import_after,"fingerprint":target.metadata.fingerprint,"origin":target.metadata.origin}})
}
pub fn read_native_conversation(config:&std::path::Path,pid:i64)->Result<String,RuntimeError> {
    let raw=std::fs::read(config.join("sessions").join(format!("{pid}.json"))).map_err(|_|failure("lifecycle_native_read"))?;
    let value:serde_json::Value=serde_json::from_slice(&raw).map_err(|_|failure("lifecycle_native_read"))?;
    let sid=value["sessionId"].as_str().filter(|sid|!sid.is_empty() && sid.bytes().all(|b|b.is_ascii_alphanumeric() || b"-_".contains(&b))).ok_or_else(||failure("lifecycle_native_read"))?;
    if value["pid"].as_i64()!=Some(pid) {return Err(failure("lifecycle_native_binding"));}
    Ok(sid.into())
}
pub fn native_conversation(config:&std::path::Path,pid:i64,birth:f64,procs:&dyn ProcessView)->Result<String,RuntimeError> {
    let before=procs.recipe(pid).map_err(|_|failure("lifecycle_process_read"))?;
    if !crate::terminal_facts::same_birth(before.birth,birth) {return Err(failure("lifecycle_process_changed"));}
    let sid=read_native_conversation(config,pid)?;
    if procs.recipe(pid).map_err(|_|failure("lifecycle_process_read"))?!=before {return Err(failure("lifecycle_native_binding"));}
    Ok(sid)
}
pub fn launched_agent(procs:&dyn ProcessView,root:i64,_conversation:&str)->Result<(i64,f64),RuntimeError> {
    let children=procs.children(Duration::ZERO).map_err(|_|failure("lifecycle_process_read"))?;
    let mut pending=vec![(root,0usize)];let mut found=Vec::new();let mut seen=std::collections::HashSet::new();
    while let Some((pid,depth))=pending.pop() {
        if !seen.insert(pid) {continue;}
        if let Ok(recipe)=procs.recipe(pid) {
            if let Ok(sid)=super::claude_relaunch::Recipe::argument_conversation(&recipe)
                && super::claude_relaunch::Recipe::capture(procs,root,pid,recipe.birth,&sid,false).is_ok() {found.push((depth,pid,recipe.birth));}
        }
        pending.extend(children.get(&pid).into_iter().flatten().map(|pid|(*pid,depth+1)));
    }
    let depth=found.iter().map(|(depth,_,_)|*depth).min().ok_or_else(||failure("relaunch_agent_identity"))?;
    let owners:Vec<_>=found.iter().filter(|(at,_,_)|*at==depth).collect();
    if owners.len()!=1 {return Err(failure("relaunch_agent_identity"));}
    Ok((owners[0].1,owners[0].2))
}
pub async fn permission_mode(io:&dyn TerminalIo,binding:&TerminalBinding)->Result<String,RuntimeError> {
    pane(io,binding).await?;
    let output=mux(io,binding,vec!["capture-pane".into(),"-p".into(),"-t".into(),binding.pane.clone()]).await?;
    if !output.success {return Err(failure("relaunch_mode_read"));}
    let text=std::str::from_utf8(&output.stdout).map_err(|_|failure("relaunch_mode_read"))?;
    crate::state::permission::parse_permission_mode(text).map(str::to_owned).ok_or_else(||failure("relaunch_mode_read"))
}

/// Pasta dos sidecars do Claude sem terminal. Nos testes da unidade é uma pasta do processo, nunca o
/// HOME real: a suíte rodada na máquina de uso não pode deixar sessão-fantasma.
pub(crate) fn claude_sidecar_dir()->Result<std::path::PathBuf,RuntimeError> {
    let home=if cfg!(test) {Some(std::env::temp_dir().join(format!("hangar-unit-{}",std::process::id())))} else {std::env::home_dir()};
    home.map(|home|home.join(".hangar/claude-headless")).ok_or_else(||failure("lifecycle_sidecar_path"))
}

/// O sidecar da sessão não existe (só a ausência; ilegível não conta): o fechamento pelo nome o apaga
/// antes de matar o cano.
pub(super) async fn claude_sidecar_absent(name:&str)->bool {
    let Ok(path)=claude_sidecar_dir().map(|dir|super::codex_sidecar::path(&dir,name)) else {return false};
    tokio::task::spawn_blocking(move||sidecar_absent_by(&path,super::queue::read_replaced)).await.unwrap_or(false)
}
/// Uma leitura que tolera a troca (`read_replaced`): no Windows quem lê no instante da troca recebe
/// NotFound, e soltar a sessão por isso a deixaria órfã.
fn sidecar_absent_by(path:&std::path::Path,read:impl FnOnce(&std::path::Path)->std::io::Result<Vec<u8>>)->bool {
    matches!(read(path),Err(error) if error.kind()==std::io::ErrorKind::NotFound)
}

/// O arquivo já existe: esta operação nunca cadastra nem prepara uma conta.
/// O chamador segura a lease do Store e o portão de revisão do ator.
pub fn delete_sidecar(target:&super::protocol::RuntimeTarget,conversation:&str)->Result<(),RuntimeError> {
    if target.provider!="claude" || target.name.is_empty() || !target.name.bytes().all(|b|b.is_ascii_alphanumeric() || b"._-".contains(&b)) {return Err(failure("lifecycle_sidecar_path"));}
    let dir=claude_sidecar_dir()?;
    super::codex_sidecar::locked(&dir,&target.name,|path|Ok(delete_locked(path,target,conversation))).map_err(|_|failure("lifecycle_sidecar_lock"))?
}
fn delete_locked(path:&std::path::Path,target:&super::protocol::RuntimeTarget,conversation:&str)->Result<(),RuntimeError> {
    let bytes=match super::queue::read_replaced(path) {Ok(bytes)=>bytes,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(()),Err(_)=>return Err(failure("lifecycle_sidecar_read"))};
    let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|failure("lifecycle_sidecar_read"))?;
    if value["key"]!=target.key || value["session_id"]!=conversation || value["name"]!=target.name
        || value["cano"]["pid"].as_u64().is_some_and(|pid|pid!=u64::from(target.binding.pid)) {return Err(failure("lifecycle_sidecar_changed"));}
    std::fs::remove_file(path).map_err(|_|failure("lifecycle_sidecar_delete"))
}

fn sidecar_hash(bytes:&[u8])->String {ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|byte|format!("{byte:02x}")).collect()}
pub(super) fn sidecar_digest(target:&super::protocol::RuntimeTarget)->Result<String,RuntimeError> {
    if target.provider!="claude" || target.name.is_empty() || !target.name.bytes().all(|b|b.is_ascii_alphanumeric() || b"._-".contains(&b)) {return Err(failure("lifecycle_sidecar_path"));}
    let path=claude_sidecar_dir()?.join(format!("{}.json",target.name));
    let bytes=super::queue::read_replaced(&path).map_err(|_|failure("lifecycle_sidecar_read"))?;Ok(sidecar_hash(&bytes))
}
/// Sob a trava `<nome>.lock` do arquivo, a mesma que o Python segura ao gravá-lo.
pub fn patch_sidecar(target:&super::protocol::RuntimeTarget,patch:&serde_json::Value)->Result<serde_json::Value,RuntimeError> {
    if target.provider!="claude" || !target.name.bytes().all(|b|b.is_ascii_alphanumeric() || b"._-".contains(&b)) || target.name.is_empty() {return Err(failure("lifecycle_sidecar_path"));}
    let dir=claude_sidecar_dir()?;
    super::codex_sidecar::locked(&dir,&target.name,|path|Ok(patch_locked(path,target,patch))).map_err(|_|failure("lifecycle_sidecar_lock"))?
}
fn patch_locked(path:&std::path::Path,target:&super::protocol::RuntimeTarget,patch:&serde_json::Value)->Result<serde_json::Value,RuntimeError> {
    use serde_json::{Value,json};
    let bytes=super::queue::read_replaced(path).map_err(|_|failure("lifecycle_sidecar_read"))?;
    let mut meta:Value=serde_json::from_slice(&bytes).map_err(|_|failure("lifecycle_sidecar_read"))?;
    let recorded_pid=if meta["cano"].is_null() {0} else {meta["cano"]["pid"].as_u64().ok_or_else(||failure("lifecycle_sidecar_read"))?};
    if meta["key"]!=target.key || meta["session_id"]!=target.metadata["session_id"] || meta["name"]!=target.name
        || recorded_pid!=u64::from(target.binding.pid) {return Ok(json!({"stale":true}));}
    let fields=patch.as_object().ok_or_else(||failure("lifecycle_sidecar_patch"))?;
    if fields.keys().any(|k|!matches!(k.as_str(),"session_id"|"permission_mode"|"previous_non_plan"|"cano"|"problema")) {return Err(failure("lifecycle_sidecar_patch"));}
    meta.as_object_mut().ok_or_else(||failure("lifecycle_sidecar_read"))?.extend(fields.clone());
    let written=serde_json::to_vec(&meta).map_err(|_|failure("lifecycle_sidecar_write"))?;
    super::queue::atomic_write(path,&written).map_err(|_|failure("lifecycle_sidecar_write"))?;
    Ok(json!({"updated":true,"metadata":meta,"digest":sidecar_hash(&written)}))
}

pub async fn stage(io:&dyn TerminalIo,binding:&TerminalBinding,endpoint:&str)->Result<(String,i64),RuntimeError> {
    let exe=std::env::current_exe().map_err(|_|failure("relaunch_binary"))?;
    let executable=exe.to_str().ok_or_else(||failure("relaunch_binary"))?;
    let mut command=[executable,super::claude_relaunch::RELAUNCH_FLAG,endpoint].iter().map(|arg|super::claude_relaunch::quote(arg,binding.windows)).collect::<Vec<_>>().join(" ");
    if !binding.windows {command=format!("exec {command}");}
    let mut args:Vec<String>=vec!["new-window".into(),"-d".into(),"-P".into(),"-F".into(),
        "#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_id}\t#{pane_pid}".into(),"-t".into(),format!("={}",binding.name)];
    // Em argumentos separados o tmux roda direto, sem o `default-shell` do usuário (o fish carrega o `conf.d`).
    if !binding.windows {args.extend(["/bin/sh".into(),"-c".into()]);}
    args.push(command);
    let output=mux(io,binding,args).await?;
    if !output.success {return Err(failure("relaunch_stage"));}
    let raw=std::str::from_utf8(&output.stdout).map_err(|_|failure("relaunch_stage"))?;
    let fields:Vec<_>=raw.trim_end().split('\t').collect();
    if fields.len()!=5 || fields[0]!=binding.name || !fields[1..3].iter().all(|s|!s.is_empty() && s.bytes().all(|b|b.is_ascii_digit())) {return Err(failure("relaunch_stage"));}
    let pid=fields[4].parse::<i64>().ok().filter(|p|*p>0).ok_or_else(||failure("relaunch_stage"))?;
    let target=if binding.windows {format!("={}:{}.{}",binding.name,fields[1],fields[2])} else {fields[3].to_owned()};
    if !binding.windows && !target.strip_prefix('%').is_some_and(|s|!s.is_empty() && s.bytes().all(|b|b.is_ascii_digit())) {return Err(failure("relaunch_stage"));}
    Ok((target,pid))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_input::{CommandOutput,IoFuture,NativeMessage,WriteOutcome};
    struct Io {lines:std::sync::Mutex<std::collections::VecDeque<CommandOutput>>}
    impl TerminalIo for Io {
        fn command<'a>(&'a self,_:CommandRequest)->IoFuture<'a,CommandOutput> {
            Box::pin(async move {self.lines.lock().unwrap().pop_front().ok_or(crate::terminal_input::IoFailure {code:"missing_fake",may_have_written:false})})
        }
        fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>)->IoFuture<'a,WriteOutcome> {Box::pin(async {panic!("lifecycle não entrega entrada")})}
    }
    fn binding()->TerminalBinding {
        TerminalBinding {name:"session".into(),pane:"%2".into(),conversation:"sid".into(),generation:4,created:1000,mux_argv:vec!["tmux".into()],windows:false,clipboard_lock_path:None}
    }
    /// A relançada não passa pelo `default-shell` do usuário: argumentos separados, o tmux roda direto.
    #[tokio::test]
    async fn the_relaunch_window_does_not_go_through_the_user_default_shell() {
        struct Seen(std::sync::Mutex<Vec<CommandRequest>>);
        impl TerminalIo for Seen {
            fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput> {
                self.0.lock().unwrap().push(r);
                Box::pin(async {Ok(CommandOutput {success:true,stdout:b"session\t0\t1\t%3\t42\n".to_vec()})})
            }
            fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>)->IoFuture<'a,WriteOutcome> {Box::pin(async {panic!("lifecycle não entrega entrada")})}
        }
        let io=Seen(std::sync::Mutex::new(vec![]));
        stage(&io,&binding(),"http://127.0.0.1:1/x").await.unwrap();
        let args=io.0.lock().unwrap()[0].args.clone();
        let tail=&args[args.len()-3..];
        assert_eq!((tail[0].as_str(),tail[1].as_str()),("/bin/sh","-c"),"{args:?}");
        assert!(tail[2].starts_with("exec ")&&tail[2].contains(super::super::claude_relaunch::RELAUNCH_FLAG),"{args:?}");
    }
    /// O `kill-session` só avisa o agente: ele leva um instante para sair, e a leitura logo depois ainda o vê
    /// vivo. Isso não é sobreviver ao fechamento.
    #[tokio::test]
    async fn an_agent_that_takes_a_moment_to_exit_after_kill_session_is_closed() {
        struct Procs {calls:std::sync::atomic::AtomicU32}
        impl ProcessView for Procs {
            fn children(&self,_:Duration)->std::io::Result<std::sync::Arc<crate::list::procs::ChildrenMap>> {Ok(std::sync::Arc::new([(10,vec![11])].into()))}
            fn cwd(&self,_:i64)->Option<std::path::PathBuf> {None}
            fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
            fn start_time(&self,_:i64)->Option<f64> {None}
            fn recipe(&self,pid:i64)->std::io::Result<crate::list::procs::ProcessRecipe> {
                // Duas leituras antes do kill; logo depois, uma ilegível (o agente no meio da saída) e uma ainda viva.
                let call=self.calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                if call==2 {return Err(std::io::ErrorKind::PermissionDenied.into());}
                let alive=call<4;
                Ok(crate::list::procs::ProcessRecipe {pid,birth:if alive {7.0} else {8.0},executable:"claude".into(),cwd:"/".into(),argv:vec![],environ:vec![],cgroup:None})
            }
        }
        let line=|| CommandOutput {success:true,stdout:b"session\t%2\t1000\t10\t0\t0\t0".to_vec()};
        let io=Io {lines:std::sync::Mutex::new([line(),line(),CommandOutput {success:true,stdout:vec![]}].into())};
        let procs=Procs {calls:Default::default()};
        assert_eq!(kill_terminal(&io,&procs,&binding(),11,7.0).await.map_err(|e|e.code),Ok(()));
    }
    #[tokio::test]
    async fn missing_capture_is_not_definitive_death() {
        let io=Io {lines:std::sync::Mutex::new([CommandOutput {success:false,stdout:vec![]}].into())};
        assert!(pane(&io,&binding()).await.is_err());
    }
    #[tokio::test]
    async fn replacement_pane_does_not_authorize_old_life_cleanup() {
        let io=Io {lines:std::sync::Mutex::new([CommandOutput {success:true,stdout:b"session\t%3\t1000\t22\t0\t0\t0".to_vec()}].into())};
        assert!(pane(&io,&binding()).await.is_err());
    }
    #[test]
    fn retry_after_close_uses_the_same_intent_and_preserves_unknown() {
        use super::super::queue::{State,Store,Action,Status};
        use super::super::protocol::ClockSample;
        use serde_json::json;
        let dir=tempfile::tempdir().unwrap();
        let binding=InteractionBinding {key:"A".into(),conversation:"sid".into(),generation:4};
        let mut state=State::new("A",4,"session",vec![json!({"id":"input","text":"pending","ts":1.0,"delivered":true,"confirmed":false})]);
        // A trava aponta para uma escrita incerta que existe; sem ela, a trava sairia na primeira `Finish`.
        state.operations.insert("input".into(),serde_json::from_value(json!({"id":"input","entry_id":"input","status":"unknown","result":{},"wire_attempts":{},
            "payload":{"kind":"input","payload":{"text":"pending","_terminal_generation":4}},"dispatch_cursor":{"conversation":"sid"}})).unwrap());
        state.runtime_state["terminal_write_barrier"]=json!({"operation_id":"input","conversation":"sid","generation":4});
        let mut store=Store::open(&dir.path().join("state.json"),dir.path(),state).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1000.0};
        let intent=json!({"operation_id":"restart","key":"A","conversation":"sid","from_generation":4,"target_generation":5});
        store.exec(4,"intent",clock,Action::AdvanceLifecycle {generation:4,state:json!({"claude_lifecycle_intent":intent})}).unwrap();
        store.exec(4,"spawn1.prepare",clock,Action::Prepare {id:"spawn1".into(),payload:json!({"logical_id":"restart"}),entry_id:None}).unwrap();
        store.exec(4,"spawn1.failed",clock,Action::Finish {id:"spawn1".into(),status:Status::Unknown,result:json!({"error_code":"fake_spawn_failed"})}).unwrap();
        // O ator foi fechado; a lease/Store e a intenção são os mesmos, não um handle inventado.
        assert_eq!(retry_fence(&binding,store.state(),"restart",1,false,None).unwrap(),(4,5));
        store.exec(4,"spawn2.life",clock,Action::AdvanceLifecycle {generation:5,state:json!({"metadata":{"key":"A","session_id":"sid"}})}).unwrap();
        assert_eq!(retry_fence(&binding,store.state(),"restart",2,false,None).unwrap(),(5,5));
        assert_eq!(store.state().operations["spawn1"].status,Status::Unknown);
        assert_eq!(store.state().rows.len(),1);
        assert_eq!(store.state().runtime_state["terminal_write_barrier"]["operation_id"],"input");
    }
    /// Ausente é só o NotFound da leitura que já tolera a troca (`read_replaced`): uma leitura, sem repetir.
    #[test]
    fn only_a_missing_sidecar_is_absent_after_one_read() {
        let calls=std::cell::Cell::new(0);
        let read=|answer:Option<std::io::ErrorKind>| { let calls=&calls; move |_:&std::path::Path| { calls.set(calls.get()+1); answer.map_or_else(||Ok(b"{}".to_vec()),|kind|Err(kind.into())) } };
        let path=std::path::Path::new("cl.json");
        use std::io::ErrorKind::{NotFound,PermissionDenied};
        assert!(sidecar_absent_by(path,read(Some(NotFound))),"ausente: fechada pelo nome");
        assert!(!sidecar_absent_by(path,read(Some(PermissionDenied))),"recusado não é ausente");
        assert!(!sidecar_absent_by(path,read(None)),"presente");
        assert_eq!(calls.get(),3,"uma leitura por consulta");
    }
    #[test]
    fn replacement_b_or_explicit_close_prevents_second_spawn_admission() {
        let a=InteractionBinding {key:"A".into(),conversation:"sid".into(),generation:4};
        let b=InteractionBinding {key:"B".into(),conversation:"new".into(),generation:1};
        let state=super::super::queue::State::new("A",4,"session",vec![]);
        assert_eq!(retry_fence(&a,&state,"restart",1,false,Some(&b)).unwrap_err().code,"runtime_binding");
        assert_eq!(retry_fence(&a,&state,"restart",1,true,None).unwrap_err().code,"lifecycle_cancelled");
        assert_eq!(retry_fence(&a,&state,"other",1,false,None).unwrap_err().code,"lifecycle_intent_changed");
    }
    #[tokio::test]
    async fn cancellation_marks_only_its_lifecycle_and_ingress_guard_drops_once() {
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let a=Barrier::default();let b=Barrier::default();
        let ca=Arc::new(AtomicBool::new(false));let cb=Arc::new(AtomicBool::new(false));
        *a.operation.lock().unwrap()=Some(("same-name".into(),ca.clone()));
        *b.operation.lock().unwrap()=Some(("other".into(),cb.clone()));
        a.cancel();assert!(ca.load(Ordering::Acquire));assert!(!cb.load(Ordering::Acquire));
        let gates=super::super::ingress::IngressGates::default();
        let pass=gates.enter("session",Duration::from_secs(1)).await.unwrap();
        let g=gates.clone();let task=tokio::spawn(async move {ClosedIngress::acquire(&g,"session").await.unwrap()});
        tokio::task::yield_now().await;
        assert!(!task.is_finished());drop(pass);
        let held=task.await.unwrap();
        assert!(gates.enter("session",Duration::ZERO).await.is_err());
        drop(held);assert!(gates.enter("session",Duration::ZERO).await.is_ok());
    }
    #[test]
    fn the_actor_can_persist_and_clear_the_problem_it_reads_from_the_sidecar() {
        use super::super::protocol::{RuntimeTarget,CanoBinding};
        use serde_json::json;
        let dir=claude_sidecar_dir().unwrap();std::fs::create_dir_all(&dir).unwrap();
        let name="problem-patch";
        std::fs::write(dir.join(format!("{name}.json")),json!({"key":"K","session_id":"sid","name":name,"cano":{"pid":42},"problema":["headless_nao_subiu","x"]}).to_string()).unwrap();
        let target=RuntimeTarget {key:"K".into(),generation:1,name:name.into(),provider:"claude".into(),metadata:json!({"session_id":"sid"}),
            binding:CanoBinding {pid:42,escuta:String::new(),token:String::new(),versao:2},lease_path:dir.join("l"),state_path:dir.join("s"),
            projection_dir:dir.join("p"),transcript:dir.join("t"),created:0.0};
        let fell=patch_sidecar(&target,&json!({"problema":["headless_processo_caiu","rc=-9"]})).unwrap();
        assert_eq!(fell["metadata"]["problema"],json!(["headless_processo_caiu","rc=-9"]));
        let cleared=patch_sidecar(&target,&json!({"problema":null})).unwrap();
        assert_eq!((cleared["updated"].clone(),cleared["metadata"]["problema"].clone()),(json!(true),json!(null)));
        assert_eq!(patch_sidecar(&target,&json!({"name":"other"})).unwrap_err().code,"lifecycle_sidecar_patch");
    }
    #[test]
    fn late_a_cannot_match_b_by_name_alone() {
        let target=crate::interactions::InteractionBinding {key:"B".into(),conversation:"new".into(),generation:5};
        assert!(!matches_life(&target,"A","old",4));
        assert!(!matches_life(&target,"B","old",5));
        assert!(!matches_life(&target,"B","new",4));
        assert!(matches_life(&target,"B","new",5));
    }
}
