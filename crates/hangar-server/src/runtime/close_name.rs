//! Encerrar pelo nome o que não tem vida aberta no Rust (o `registry.kill` do Python): Codex e Claude sem
//! terminal parados, Claude cujo agente morreu, pane de outro programa e a sessão que já acabou, que é
//! sucesso sem `kill-session`. O processo ou o pane que não sai deixa sidecar e fila como estavam.
use std::ffi::{OsStr,OsString};
use std::future::Future;
use std::path::{Path,PathBuf};
use std::time::Duration;

use serde_json::{Value,json};

use super::codex_sidecar::{self,Meta};
use super::gateway::RuntimeRegistry;
use super::lifecycle::ClosedIngress;
use super::process::{self,Liveness};
use super::protocol::RuntimeError;
use crate::list::discover_other::truthy;
use crate::list::procs::{ProcessView,SystemProcs,exited};
use crate::session_open::open::NameClaim;
use crate::state::capture::{MuxProcess,TIMEOUT};

/// O processo ou o pane da sessão não saiu: nada durável foi apagado.
pub const KILL_UNCONFIRMED:&str = "session_kill_unconfirmed";
/// A sessão saiu, mas a fila dela não: a próxima com o nome a herdaria.
pub const QUEUE_CLEAR_FAILED:&str = "session_queue_clear_failed";
/// Outra abertura ou fechamento do mesmo nome está em curso.
pub const NAME_BUSY:&str = "erro_nome_em_uso";

/// Pastas e multiplexador da lista, para quando a vigia do Codex não está ligada.
pub struct NameEnv { pub home:PathBuf, pub queue_dir:PathBuf, pub mux_program:OsString }

/// O nome e a porta de entrada da sessão, seguros do começo ao fim do fechamento.
pub struct Closing { _claim:NameClaim, _ingress:ClosedIngress }

pub(super) fn unconfirmed(message:&str) -> RuntimeError { RuntimeError::new(KILL_UNCONFIRMED,message) }

fn mux_silent() -> RuntimeError { RuntimeError::new("close_kill_mux","o multiplexador não respondeu; nada foi encerrado") }

fn queue_failed() -> RuntimeError { RuntimeError::new(QUEUE_CLEAR_FAILED,"a sessão foi encerrada, mas a fila dela não foi apagada") }

impl RuntimeRegistry {
    /// Antes de qualquer encerramento: a abertura do mesmo nome e as escritas esperam (ou recusam) até o fim.
    /// `fallback_home`: a casa da lista, sem a vigia.
    pub async fn begin_close(&self,fallback_home:Option<PathBuf>,name:&str) -> Result<Closing,RuntimeError> {
        let home = self.watcher_ref().map(|watcher|watcher.cfg.home.clone()).or(fallback_home);
        let claim = match home {
            Some(home)=>NameClaim::take(&home,name).map_err(|_|RuntimeError::new(NAME_BUSY,"outra abertura ou fechamento deste nome está em curso"))?,
            None=>NameClaim::default(),
        };
        let ingress = ClosedIngress::acquire_unless_transfer(self.ingress(),name).await?;
        Ok(Closing { _claim:claim,_ingress:ingress })
    }

    /// `known`: a conversa da vida guardada; `lookup` (a da lista) só roda quando há o que fechar, antes do
    /// kill. O `Ok` leva `transcript` quando ela é conhecida, para o nome fechado, e `killed` só com processo
    /// ou pane desta sessão de fato encerrado.
    pub async fn close_by_name(&self,fallback:Option<NameEnv>,name:&str,known:Option<PathBuf>,
        lookup:impl Future<Output=Option<PathBuf>>) -> Result<Value,RuntimeError> {
        let env = match self.watcher_ref() {
            Some(watcher)=>NameEnv { home:watcher.cfg.home.clone(),queue_dir:watcher.cfg.queue_dir.clone(),mux_program:watcher.cfg.mux_program.clone() },
            None=>fallback.ok_or_else(||RuntimeError::new("session_close_unavailable","as pastas das sessões não são conhecidas; nada foi encerrado"))?,
        };
        let _ingress = ClosedIngress::acquire_unless_transfer(self.ingress(),name).await?;
        let dir = env.home.join(".hangar").join("claude-headless");
        let parked = read(&dir,name).await?.is_some_and(|meta|meta.get("headless").is_some_and(truthy));
        let mux = MuxProcess::new(&env.mux_program,TIMEOUT);
        let present = parked || mux.has_session(name).await.ok_or_else(mux_silent)?;
        let transcript = match known { Some(path)=>Some(path), None if present=>lookup.await, None=>None };
        let killed = if parked {
            let killed = stop_parked(&dir,name,&env.queue_dir).await?;
            // A rota `/shell` cria o `term-<nome>` também para a sessão sem terminal.
            kill_hidden(&mux,name).await;
            killed
        } else { kill_pane(&mux,name,present).await?; present };
        // Como o Python: a pergunta aberta desta conversa não reabre numa sessão futura com o mesmo nome.
        if let Some(path) = transcript.as_deref().and_then(crate::state::ask::sidecar_path)
            && let Err(error) = std::fs::remove_file(&path) && error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(session=%name,kind=?error.kind(),"pergunta aberta não foi apagada ao fechar");
        }
        forget(&env.queue_dir,name).await?;
        let mut value = json!({"closed":true,"killed":killed});
        if let Some(path) = transcript { value["transcript"] = json!(path.to_string_lossy()); }
        Ok(value)
    }
}

/// Codex sem terminal, aberto aqui ou parado: o ator sai, o cano morre e sidecar, rastros, fila e o shell
/// marcado `term-<nome>` saem.
pub(super) async fn close_codex_headless(registry:&RuntimeRegistry,home:&Path,queue_dir:&Path,mux_program:&OsStr,name:&str) -> Result<Value,RuntimeError> {
    let _ingress = ClosedIngress::acquire_unless_transfer(registry.ingress(),name).await?;
    if let Some((key,generation,_)) = registry.codex_headless(name).await {
        registry.close_with_kill(&key,generation).await?;
    }
    stop_parked(&codex_sidecar::dir(home),name,queue_dir).await?;
    kill_hidden(&MuxProcess::new(mux_program,TIMEOUT),name).await;
    forget(queue_dir,name).await?;
    Ok(json!({"closed":true,"killed":true}))
}

/// `kill-session` confirmado e o shell `term-<nome>` só com a nossa marca; ausente não mata nada.
async fn kill_pane(mux:&MuxProcess,name:&str,present:bool) -> Result<(),RuntimeError> {
    if present {
        match mux.kill_session(name).await {
            Some(true)=>{},
            Some(false)=>return Err(unconfirmed("a sessão continua de pé depois do kill-session; nada foi apagado")),
            None=>return Err(mux_silent()),
        }
    }
    kill_hidden(mux,name).await;
    Ok(())
}

/// O shell do painel (`term-<nome>`) sai só com a marca de que é nosso: sem ela é de outro.
async fn kill_hidden(mux:&MuxProcess,name:&str) {
    let hidden = format!("term-{name}");
    if mux.is_hidden(&hidden).await && mux.kill_session(&hidden).await != Some(true) {
        tracing::warn!(session=%name,"shell escondido da sessão não fechou");
    }
}

async fn read(dir:&Path,name:&str) -> Result<Option<Meta>,RuntimeError> {
    let path = codex_sidecar::path(dir,name);
    tokio::task::spawn_blocking(move||codex_sidecar::read(&path)).await
        .map_err(|_|RuntimeError::new("close_sidecar_unreadable","a leitura do arquivo da sessão falhou; nada foi encerrado"))?
        .map_err(|_|RuntimeError::new("close_sidecar_unreadable","o arquivo da sessão não pôde ser lido; nada foi encerrado"))
}

/// O cano e os filhos diretos dele (o agente), com o nascimento de cada um. Neto que saiu do grupo não
/// conta: o Python esperava só o cano, e um processo solto seguraria o fechamento para sempre.
async fn cano_and_agent(pid:i64) -> Result<Vec<(i64,f64)>,()> {
    tokio::task::spawn_blocking(move || {
        let procs = SystemProcs::default();
        let children = procs.children(Duration::ZERO).map_err(|_|())?;
        let mut lives = Vec::new();
        for pid in std::iter::once(pid).chain(children.get(&pid).into_iter().flatten().copied()) {
            match procs.recipe(pid) {
                Ok(recipe)=>lives.push((pid,recipe.birth)),
                Err(error) if exited(&error) || error.kind() == std::io::ErrorKind::NotFound=>{},
                Err(_)=>return Err(()),
            }
        }
        Ok(lives)
    }).await.map_err(|_|())?
}

/// O cano do sidecar (relido agora: o ator pode tê-lo trocado) morre com o agente; só então o sidecar e a
/// fila da chave saem. Pid que já não é o cano da chave não leva sinal. `Ok(true)`: matou o cano desta sessão.
async fn stop_parked(dir:&Path,name:&str,queue_dir:&Path) -> Result<bool,RuntimeError> {
    let Some(meta) = read(dir,name).await? else { return Ok(false) };
    let key = meta.get("key").and_then(Value::as_str).filter(|key|!key.is_empty()).map(str::to_owned);
    let mut killed = false;
    if let Some(cano) = meta.get("cano").filter(|cano|!cano.is_null()) {
        let cano:process::Cano = serde_json::from_value(cano.clone())
            .map_err(|_|unconfirmed("o processo gravado da sessão não pôde ser lido; nada foi apagado"))?;
        let key = key.clone().ok_or_else(||unconfirmed("sem a chave da sessão o processo dela não se comprova; nada foi apagado"))?;
        let (pid,owned) = (cano.pid,key.clone());
        killed = tokio::task::spawn_blocking(move||process::liveness(pid,&owned) == Liveness::Ours).await.unwrap_or(true);
        let lives = if killed {
            cano_and_agent(i64::from(pid)).await.map_err(|()|unconfirmed("os processos da máquina não puderam ser lidos; nada foi apagado"))?
        } else { Vec::new() };
        process::kill(&cano,&key,dir).await.map_err(|_|unconfirmed("o processo da sessão não saiu; sidecar e fila conservados"))?;
        let left = super::codex_relaunch::wait_absent(lives).await;
        if !left.is_empty() {
            let pids:Vec<String> = left.iter().map(i64::to_string).collect();
            return Err(unconfirmed(&format!("processos da sessão seguem vivos ({}); sidecar e fila conservados",pids.join(", "))));
        }
    }
    let path = codex_sidecar::path(dir,name);
    match tokio::task::spawn_blocking(move||std::fs::remove_file(path)).await {
        Ok(Ok(())) => {},
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {},
        _ => return Err(RuntimeError::new("close_sidecar_io","o processo saiu, mas o arquivo da sessão não pôde ser apagado")),
    }
    if let Some(key) = key.filter(|key|super::codex_terminal::valid_key(key)) {
        let queue_dir = queue_dir.to_owned();
        let discarded = tokio::task::spawn_blocking(move || {
            let (state,lease) = super::codex_terminal::queue_files(&queue_dir,&key);
            if !state.exists() { return Ok(()); }
            super::codex_terminal::lease_blocking(&lease).and_then(|_lease|super::queue::discard(&state,&queue_dir,true))
        }).await;
        if !matches!(discarded,Ok(Ok(()))) {
            tracing::warn!(session=%name,code=QUEUE_CLEAR_FAILED,"fila da sessão encerrada não foi apagada");
            return Err(queue_failed());
        }
    }
    Ok(killed)
}

/// A fila do nome, o `then` e o loop: a próxima sessão com o nome não os herda.
async fn forget(queue_dir:&Path,name:&str) -> Result<(),RuntimeError> {
    let (base,owned) = (queue_dir.parent().unwrap_or(queue_dir).to_owned(),name.to_owned());
    let result = tokio::task::spawn_blocking(move||crate::session_open::server::forget_queue(&base,&owned)).await;
    if matches!(result,Ok(Ok(()))) { return Ok(()); }
    tracing::warn!(session=%name,code=QUEUE_CLEAR_FAILED,"fila do nome encerrado não foi apagada");
    Err(queue_failed())
}
