//! Rename de uma sessão aberta aqui, sem o Python: a entrada fecha sem matar o processo, quem chama
//! troca o nome por fora (multiplexador, sidecar), a fila recebe o nome novo na MESMA geração e a
//! sessão reabre pelo nome novo. Quem chama segura as portas dos dois nomes do começo ao fim.
use super::*;

/// A entrada fechada para o rename, com o que a fila e a reabertura precisam.
#[derive(Clone)]
pub(crate) struct Detached {
    kind: Kind,
    key: String,
    generation: u64,
    state_path: std::path::PathBuf,
    lease_path: std::path::PathBuf,
    projection_dir: std::path::PathBuf,
}

#[derive(Clone)]
enum Kind {
    Terminal(Box<super::super::terminal::TerminalTarget>),
    ClaudeHeadless,
    CodexHeadless,
}

impl RuntimeRegistry {
    /// Fecha, sem matar, a entrada aberta aqui com `name`. `None`: nenhuma; o rename segue só por fora.
    /// Cancela antes a operação de ciclo de vida do nome, como o fechamento da porta da ponte fazia.
    /// O Codex com terminal fica: a vigia dele não passa pela porta, e ela mesma segue o pane que
    /// mudou de nome com a mesma chave (fecha o ator velho e adota no nome novo, sem matar nada).
    pub(crate) async fn detach_for_rename(&self,name:&str) -> Result<Option<Detached>,RuntimeError> {
        self.cancel_lifecycle_name(name).await;
        let found=self.entries.lock().await.iter().find(|(_,entry)|entry.name==name && !matches!(entry.handle,EntryHandle::CodexTerminal {..}))
            .map(|(key,entry)|(key.clone(),entry.generation,entry.handle.clone(),entry.provider.clone(),entry.lease_path.clone()));
        let Some((key,generation,handle,provider,lease_path))=found else {return Ok(None)};
        let queue_dir=self.claude_dirs().1;
        let (kind,state_path,projection_dir)=match &handle {
            EntryHandle::Terminal {target,..}=>(Kind::Terminal(Box::new(target.clone())),target.state_path.clone(),target.projection_dir.clone()),
            EntryHandle::CodexTerminal {..}=>return Ok(None),
            EntryHandle::Headless(_)=>{
                let kind=if provider=="codex" {Kind::CodexHeadless} else {Kind::ClaudeHeadless};
                (kind,super::super::codex_terminal::queue_files(&queue_dir,&key).0,queue_dir)
            },
        };
        let barrier=self.barrier(&key).await;barrier.cancel();
        let _guard=barrier.lock().await;
        self.close_locked(&key,generation,false).await?;
        Ok(Some(Detached {kind,key,generation,state_path,lease_path,projection_dir}))
    }

    /// Fila com `name` na mesma geração e reabertura por `name`. Serve também para desfazer: com o
    /// nome antigo, a sessão volta à entrada de antes.
    pub(crate) async fn reattach_after_rename(&self,detached:Detached,name:&str) -> Result<(),RuntimeError> {
        let Detached {kind,key,generation,state_path,lease_path,projection_dir}=detached;
        rename_queue(&state_path,&projection_dir,&lease_path,&key,generation,name).await?;
        let missing=||failure("runtime_binding");
        match kind {
            Kind::Terminal(target)=>{
                let mut next=*target;
                if next.binding.windows {
                    // No psmux o alvo do pane leva o nome (`=<nome>:<janela>.<pane>`).
                    let rest=next.binding.pane.split_once(':').map(|(_,rest)|rest.to_owned()).ok_or_else(||failure("terminal_binding"))?;
                    next.binding.pane=format!("={name}:{rest}");
                }
                next.name=name.into();next.binding.name=name.into();
                let barrier=self.barrier(&key).await;let _guard=barrier.lock().await;
                self.open_terminal_locked(next.clone()).await?;
                self.rebind_claude_hub(&next)?;
            },
            Kind::ClaudeHeadless=>self.open_claude_headless(name,false).await.ok_or_else(missing)??,
            Kind::CodexHeadless=>self.watcher.get().cloned().ok_or_else(missing)?.open_parked(self,name).await.ok_or_else(missing)??,
        }
        Ok(())
    }
}

impl RuntimeRegistry {
    /// Fila da sessão sem terminal parada (sem entrada aqui): o arquivo de estado da chave recebe o nome
    /// novo na geração dele. `false`: a chave não tem fila.
    pub(crate) async fn rename_parked_queue(&self,key:&str,name:&str) -> Result<bool,RuntimeError> {
        let queue_dir=self.claude_dirs().1;
        let (state_path,lease_path)=super::super::codex_terminal::queue_files(&queue_dir,key);
        let read={let state_path=state_path.clone();tokio::task::spawn_blocking(move||super::super::queue::read_replaced(&state_path)).await.map_err(|_|failure("queue_job"))?};
        let generation=match read {
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(false),
            Err(_)=>return Err(failure("queue_io")),
            Ok(raw)=>serde_json::from_slice::<Value>(&raw).ok().and_then(|state|state["generation"].as_u64()).ok_or_else(||failure("queue_io"))?,
        };
        rename_queue(&state_path,&queue_dir,&lease_path,key,generation,name).await?;
        Ok(true)
    }
}

/// `Action::Rename` sob a trava da fila, sem avançar a geração; a projeção `<nome>.jsonl` acompanha.
async fn rename_queue(state_path:&std::path::Path,projection_dir:&std::path::Path,lease_path:&std::path::Path,key:&str,generation:u64,name:&str) -> Result<(),RuntimeError> {
    let lease=wait_lease(lease_path).await?;
    let (state_path,projection_dir,key,name)=(state_path.to_owned(),projection_dir.to_owned(),key.to_owned(),name.to_owned());
    tokio::task::spawn_blocking(move || -> Result<(),RuntimeError> {
        let _lease=lease;
        let mut store=Store::open(&state_path,&projection_dir,QueueState::new(&key,generation,&name,vec![])).map_err(|_|failure("queue_io"))?;
        if store.state().owner_key!=key || store.state().generation!=generation {return Err(failure("runtime_generation"));}
        let epoch_s=SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64());
        store.exec(generation,&format!("rename:{key}:{generation}:{name}:{epoch_s}"),ClockSample {monotonic_s:0.0,epoch_s},Action::Rename {name}).map_err(|_|failure("queue_io"))?;
        store.ensure_projection().map_err(|_|failure("queue_io"))?;
        Ok(())
    }).await.map_err(|_|failure("queue_job"))?
}
