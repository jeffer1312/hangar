//! Executor terminal serial; diário, posse e confirmação são os mesmos da fila.
use super::{actor::PolicyClient,protocol::{ClockSample,Disposition,RequestId,RuntimeCommand,RuntimeError,RuntimeEvent,RuntimeReply},queue::{Action,QueueActor,Status},receipt::{DispatchCursor,ReceiptIndex}};
use crate::terminal_input::{self as input,TerminalBinding,TerminalDriver,TerminalIo,TerminalServices,InputFacts,InputLimits,PluginRequest,PluginReply,ServiceFuture,ServiceError,DeliveryResult,QuestionAnswer,AnswerKind};
use crate::mods::click::{Pane,PaneFuture,PaneOp,PaneReply};
use serde_json::{Value,json};
use std::{collections::VecDeque,path::PathBuf,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}},time::{Duration,SystemTime,UNIX_EPOCH}};
use tokio::sync::{Mutex,mpsc,oneshot,broadcast};

#[derive(Clone)]
pub struct TerminalTarget {
    pub key:String,pub generation:u64,pub name:String,pub binding:TerminalBinding,
    pub lease_path:PathBuf,pub state_path:PathBuf,pub projection_dir:PathBuf,pub transcript:PathBuf,pub created:f64,
}
/// A âncora da faixa dos mods (`mods::tree::anchor`, o começo do primeiro texto dela): o elo do `Mods` a
/// escreve a cada `/ui` do plugin, e o executor a lê para reconhecer na tela a faixa inteira focada.
pub type ModsAnchor=Arc<std::sync::Mutex<Option<String>>>;
/// `stall_notice`: entrega adiada sem escrita por mais que isso aparece na vista e no log. `focus_return`:
/// com uma entrada esperando e o foco num painel ou na faixa de um mod há mais que isso, sem clique em curso,
/// o executor devolve o teclado ao prompt e escreve. `clear_wait`: quanto a trava do `/clear` espera a
/// conversa nova antes de conferir se ele foi aplicado.
pub struct TerminalOptions { pub io:Arc<dyn TerminalIo>,pub limits:InputLimits,pub tick:Duration,pub stall_notice:Duration,pub anchor:ModsAnchor,pub focus_return:Duration,pub clear_wait:Duration }
impl Default for TerminalOptions {
    fn default()->Self {Self {io:Arc::new(input::ProcessIo::default()),limits:InputLimits::default(),tick:Duration::from_secs(1),stall_notice:Duration::from_secs(30),
        anchor:ModsAnchor::default(),focus_return:FOCUS_RETURN,clear_wait:CLEAR_APPLY_WAIT}}
}
/// Quanto uma linha espera, desde a primeira vez que viu o foco num mod, antes de o executor devolver o
/// teclado ao prompt. Cobre quem acabou de levar o foco a um painel no terminal, sem deixar a mensagem do
/// app parada até alguém mexer no terminal; nenhum clique do app está em curso, porque com a reserva do pane
/// a entrada nem chega aqui.
const FOCUS_RETURN:Duration=Duration::from_secs(10);
/// Quantas devoluções uma linha tenta; as que falham não viram um laço de teclas, só o registro.
const FOCUS_RETURN_TRIES:u32=2;
/// Teclas de uma devolução: o anel do `ctrl+x tab` medido tem até 23 paradas (doze botões na faixa e dez
/// painéis), com folga.
const FOCUS_RETURN_KEYS:usize=32;
const FOCUS_STEP_POLL:Duration=Duration::from_millis(20);
/// Foco de um mod visto na escrita de uma linha: quando a devolução ao prompt pode ser tentada e quantas já
/// foram. É da linha, como a série do `Stall`: a linha seguinte começa do zero.
struct Away {row:String,since:tokio::time::Instant,next:tokio::time::Instant,tries:u32}
fn error(code:&str)->RuntimeError {RuntimeError::new(code,"operação terminal conservada no diário")}
fn sample()->ClockSample {ClockSample {monotonic_s:0.0,epoch_s:SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0)}}
fn safe_characters(text:&str)->bool {!text.chars().any(|c| c.is_control() && !matches!(c,'\n'|'\t'))}
fn valid_text(text:&str)->bool {!text.trim().is_empty() && safe_characters(text)}
fn status(disposition:Disposition)->Status {match disposition {Disposition::Accepted=>Status::Accepted,Disposition::Deferred=>Status::Deferred,Disposition::Rejected=>Status::Rejected,Disposition::Unknown=>Status::Unknown}}
fn reply(id:&str,disposition:Disposition,payload:Value)->RuntimeReply {RuntimeReply {operation_id:id.into(),disposition,payload}}
fn delivery(id:&str,result:DeliveryResult)->RuntimeReply {
    let disposition=match result.disposition {input::Disposition::Accepted=>Disposition::Accepted,input::Disposition::Deferred=>Disposition::Deferred,input::Disposition::Rejected=>Disposition::Rejected,input::Disposition::Unknown=>Disposition::Unknown};
    reply(id,disposition,serde_json::to_value(result).unwrap())
}

enum Message {
    Command {id:String,kind:String,payload:Value,response:oneshot::Sender<Result<RuntimeReply,RuntimeError>>},
    Queue {id:String,action:Action,response:oneshot::Sender<Result<Value,RuntimeError>>},
    /// Operação de mod; `start_by`: depois disso ela não age (C1).
    Pane {op:PaneOp,start_by:std::time::Instant,response:oneshot::Sender<Result<PaneReply,RuntimeError>>},
    Snapshot(oneshot::Sender<Result<Value,RuntimeError>>),Drain(oneshot::Sender<Result<Value,RuntimeError>>),
    Confirm(oneshot::Sender<Result<Value,RuntimeError>>),Stop(oneshot::Sender<Result<(),RuntimeError>>),
}
type ActorTask=Arc<Mutex<Option<tokio::task::JoinHandle<Result<(),RuntimeError>>>>>;
#[derive(Clone)]
pub struct TerminalHandle {sender:mpsc::Sender<Message>,closed:Arc<AtomicBool>,task:ActorTask,stopped:Arc<Mutex<Option<Result<(),RuntimeError>>>>,anchor:ModsAnchor}
impl TerminalHandle {
    /// A âncora que este executor lê (`TerminalOptions::anchor`): o elo do `Mods` escreve nela.
    pub fn anchor(&self)->ModsAnchor {self.anchor.clone()}
    pub async fn command(&self,command:RuntimeCommand)->Result<RuntimeReply,RuntimeError> {
        let kind=serde_json::to_value(command.kind).unwrap().as_str().unwrap().to_string();
        self.control(command.operation_id,kind,command.payload).await
    }
    pub async fn control(&self,id:String,kind:String,payload:Value)->Result<RuntimeReply,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(error("runtime_stopping"));}
        let (response,receive)=oneshot::channel(); self.sender.send(Message::Command {id,kind,payload,response}).await.map_err(|_|error("runtime_closed"))?;
        receive.await.map_err(|_|error("runtime_closed"))?
    }
    pub async fn queue(&self,id:String,action:Action)->Result<Value,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(error("runtime_stopping"));}
        let (response,receive)=oneshot::channel(); self.sender.send(Message::Queue {id,action,response}).await.map_err(|_|error("runtime_closed"))?;
        receive.await.map_err(|_|error("runtime_closed"))?
    }
    /// Operação de mod no pane: entra na fila deste ator (serial com a entrada), fora do diário.
    pub async fn pane(&self,op:PaneOp,start_by:std::time::Instant)->Result<PaneReply,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(error("runtime_stopping"));}
        let (response,receive)=oneshot::channel(); self.sender.send(Message::Pane {op,start_by,response}).await.map_err(|_|error("runtime_closed"))?;
        receive.await.map_err(|_|error("runtime_closed"))?
    }
    async fn query(&self,kind:&str)->Result<Value,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(error("runtime_stopping"));}
        let (send,receive)=oneshot::channel(); let message=match kind {"drain"=>Message::Drain(send),"confirm"=>Message::Confirm(send),_=>Message::Snapshot(send)};
        self.sender.send(message).await.map_err(|_|error("runtime_closed"))?; receive.await.map_err(|_|error("runtime_closed"))?
    }
    pub async fn snapshot(&self)->Result<Value,RuntimeError> {self.query("snapshot").await}
    pub async fn drain(&self)->Result<Value,RuntimeError> {self.query("drain").await}
    pub async fn confirm(&self)->Result<Value,RuntimeError> {self.query("confirm").await}
    pub async fn ensure_projection(&self)->Result<Value,RuntimeError> {self.queue(format!("terminal-projection:{}",sample().epoch_s),Action::EnsureProjection).await}
    pub async fn stop(&self)->Result<(),RuntimeError> {
        let mut stopped=self.stopped.lock().await; if let Some(result)=&*stopped {return result.clone();}
        self.closed.store(true,Ordering::Release);
        let (send,receive)=oneshot::channel();
        let mut result=match self.sender.send(Message::Stop(send)).await {Ok(())=>receive.await.map_err(|_|error("runtime_closed")).and_then(|r|r),Err(_)=>Err(error("runtime_closed"))};
        if let Some(task)=self.task.lock().await.take() {let joined=task.await.map_err(|_|error("runtime_panic")).and_then(|r|r); if joined.is_err(){result=joined;}}
        *stopped=Some(result.clone()); result
    }
}
impl Pane for TerminalHandle {
    fn op(&self,op:PaneOp,start_by:std::time::Instant)->PaneFuture {
        let handle=self.clone();
        Box::pin(async move {handle.pane(op,start_by).await.map_err(|failure|crate::mods::model::pane_failed(&failure.code))})
    }
}

/// O clique de mod não lê os fatos do Python: confere só a identidade do pane.
struct NoFacts;
impl TerminalServices for NoFacts {
    fn facts<'a>(&'a self,_:&'a TerminalBinding)->ServiceFuture<'a,InputFacts> {Box::pin(async {Err(ServiceError("mods_no_facts"))})}
    fn publish<'a>(&'a self,_:&'a TerminalBinding,_:PluginRequest)->ServiceFuture<'a,PluginReply> {Box::pin(async {Err(ServiceError("mods_no_publish"))})}
}

struct Services {
    target:TerminalTarget,queue:Arc<QueueActor>,policy:PolicyClient,root:String,attempt:String,text:String,sequence:Arc<AtomicU64>,
}
impl Services {
    async fn action(&self,label:&str,action:Action)->Result<Value,RuntimeError> {
        let seq=self.sequence.fetch_add(1,Ordering::Relaxed);
        self.queue.exec(self.target.generation,&format!("terminal:{label}:{seq}"),sample(),action).await.map_err(|_|error("queue_io"))
    }
    async fn call(&self,kind:&str,request_id:RequestId,payload:Value)->Result<Value,RuntimeError> {
        let phase=format!("terminal-policy:{}:{}:{}",self.target.generation,self.attempt,self.sequence.fetch_add(1,Ordering::Relaxed));
        self.action("policy-prepare",Action::Prepare {id:phase.clone(),payload:json!({"kind":kind,"request_id":request_id,"payload":payload}),entry_id:None}).await?;
        self.action("policy-dispatch",Action::BeginDispatch {id:phase.clone(),wire_id:phase.clone(),staged:false}).await?;
        let result=self.policy.run_for(&self.target.key,self.target.generation,kind,&request_id,payload,&phase).await;
        self.action("policy-finish",Action::Finish {id:phase,status:if result.is_ok(){Status::Accepted}else{Status::Unknown},result:result.clone().unwrap_or_else(|e|json!({"error_code":e.code}))}).await?;
        result
    }
}
impl TerminalServices for Services {
    fn facts<'a>(&'a self,binding:&'a TerminalBinding)->ServiceFuture<'a,InputFacts> {Box::pin(async move {
        // Leitura sem efeito: fora do diário, que regravaria o estado inteiro com fsync três vezes.
        let phase=format!("terminal-facts:{}:{}:{}",self.target.generation,self.attempt,self.sequence.fetch_add(1,Ordering::Relaxed));
        let value=self.policy.run_for(&self.target.key,self.target.generation,"terminal_facts",&RequestId::String(self.root.clone()),
            json!({"binding":binding,"operation_id":self.root,"text":self.text}),&phase).await.map_err(|_|ServiceError("terminal_facts"))?;
        serde_json::from_value(value).map_err(|_|ServiceError("terminal_facts_shape"))
    })}
    fn writing<'a>(&'a self)->ServiceFuture<'a,()> {Box::pin(async move {
        self.action("writing",Action::MarkWriting {id:self.attempt.clone(),wire_id:format!("terminal:{}:{}",self.target.generation,self.attempt)}).await
            .map(|_|()).map_err(|failure|{
                tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"diário não gravou o início da escrita; a entrada espera");
                ServiceError("write_journal")
            })
    })}
    fn publish<'a>(&'a self,binding:&'a TerminalBinding,request:PluginRequest)->ServiceFuture<'a,PluginReply> {Box::pin(async move {
        let value=self.call("terminal_publish",RequestId::String(self.root.clone()),json!({"binding":binding,"operation_id":self.root,"publication":request,"generation":self.target.generation})).await.map_err(|_|ServiceError("terminal_publish"))?;
        serde_json::from_value(value).map_err(|_|ServiceError("terminal_publish_shape"))
    })}
}

struct Executor {
    target:TerminalTarget,queue:Arc<QueueActor>,policy:PolicyClient,options:TerminalOptions,
    events:broadcast::Sender<RuntimeEvent>,revision:Arc<AtomicU64>,sequence:Arc<AtomicU64>,receipt:ReceiptIndex,deliverable:bool,last_error:Option<String>,
    /// Entradas com linha na fila cuja entrega ficou incerta nesta vida do ator: o transcript ainda
    /// pode prová-las. Incerteza sem linha (tecla, seleção) não tem prova e só sai na reabertura.
    uncertain:Vec<String>,unprovable:bool,
    /// Teclado emprestado ao Python (administração que digita no pane): id, prazo e o pedido que o abriu.
    loan:Option<(String,tokio::time::Instant,String)>,
    /// Linha da fila que o terminal recusa sem escrever (composer ocupado, tela ilegível): espera
    /// crescente entre as tentativas e, passado o `stall_notice`, o motivo na vista.
    stall:Option<Stall>,
    /// Pane reservado a um clique de mod (`PaneOp::Hold`) até este instante.
    hold:Option<tokio::time::Instant>,
    /// A identidade do pane já foi conferida nesta reserva: as operações seguintes do clique não a refazem.
    hold_checked:bool,
    /// Comandos e pedidos de drenagem que chegaram com o pane reservado: saem na ordem depois do `Release`.
    parked:VecDeque<Message>,
    /// A entrada adiada porque o foco está num painel ou na faixa de um mod (`focus_guard`).
    away:Option<Away>,
    /// Trava do `/clear` erguida: quando conferir se a conversa nova apareceu (`expire_clear`).
    clear_watch:Option<tokio::time::Instant>,
    /// Até quando a vista mostra que o último `/clear` não foi aplicado.
    clear_notice:Option<tokio::time::Instant>,
    /// Linha cujo foco no rodapé o Esc não devolveu, e quantas vezes: como a devolução dos mods, o Esc
    /// tem teto e não vira laço de teclas.
    footer_misses:Option<(String,u32)>,
}
struct Stall {row:String,code:String,since:tokio::time::Instant,wait:Duration,next:tokio::time::Instant,surfaced:bool}
/// Teto da espera entre tentativas: o composer que esvazia é visto na hora, e o resto (tela
/// ilegível) não deve atrasar a mensagem muito além disso.
const MAX_STALL_WAIT:Duration=Duration::from_secs(8);
/// Esperas normais (o Claude ocupado, uma pergunta na tela) não contam como entrega parada, nem a
/// escrita desfeita (`input_unproved`), que já tem o teto de tentativas da fila, nem falha do
/// diário (`write_journal`), que tem aviso próprio.
fn stalled_code(result:&RuntimeReply)->Option<&str> {
    if result.disposition!=Disposition::Deferred || result.payload["stage"].is_null() {return None;}
    result.payload["code"].as_str().filter(|code|!matches!(*code,"question_open"|"not_ready"|"overlay"|"input_unavailable"|"input_unproved"|"write_journal"))
}
/// Teto do empréstimo: a administração mais longa (troca de modelo/motor) leva segundos.
const MAX_LOAN_S:u64=120;
/// O `clear_wait` padrão: o transcript novo do `/clear` nasce em menos de um segundo.
const CLEAR_APPLY_WAIT:Duration=Duration::from_secs(10);
/// Quanto o aviso de `/clear` não aplicado fica na vista.
const CLEAR_NOTICE:Duration=Duration::from_secs(60);
/// Até quando um transcript novo começado por `/clear` segura a trava esperando o Python trocar o
/// vínculo; depois disso só os fatos dele valem (o arquivo pode ser de outra sessão na mesma pasta).
const CLEAR_DISK_TRUST_S:f64=60.0;
/// O `/clear` só pode ter trocado a conversa se o Enter saiu: aceito, ou incerto nas etapas do Enter
/// (sem etapa é a queda no meio, que não se sabe). Incerto antes dele segue como entrega incerta comum.
fn clear_may_have_run(disposition:Disposition,payload:&Value)->bool {
    match disposition {
        Disposition::Accepted=>true,
        Disposition::Unknown=>payload["code"]!="submission_blocked" && payload["stage"].as_str().is_none_or(|stage|matches!(stage,"submit"|"submit_proof")),
        _=>false,
    }
}
/// A prova no disco de que o `/clear` rodou: um transcript nascido depois do despacho, na pasta da
/// conversa, que começa pelo registro do comando. Leitura que falha conta como prova: na dúvida a
/// trava fica.
fn clear_on_disk(transcript:&std::path::Path,since:f64)->Result<bool,std::io::Error> {
    use std::io::Read;
    const MARK:&[u8]=b"<command-name>/clear</command-name>";
    let Some(dir)=transcript.parent() else {return Ok(false)};
    let gone=|failure:&std::io::Error|failure.kind()==std::io::ErrorKind::NotFound;
    for entry in std::fs::read_dir(dir)? {
        let path=entry?.path();
        if path==transcript || path.extension().is_none_or(|e|e!="jsonl") {continue;}
        // Apagado no meio da varredura não é prova nem dúvida.
        let meta=match std::fs::metadata(&path) {Ok(meta)=>meta,Err(failure) if gone(&failure)=>continue,Err(failure)=>return Err(failure)};
        let born=meta.created().or_else(|_|meta.modified())?.duration_since(UNIX_EPOCH).map_or(0.0,|t|t.as_secs_f64());
        if born<since-1.0 {continue;}
        let mut head=Vec::new();
        match std::fs::File::open(&path).and_then(|f|f.take(16*1024).read_to_end(&mut head)) {
            Err(failure) if gone(&failure)=>continue,
            other=>{other?;}
        }
        if head.windows(MARK.len()).any(|w|w==MARK) {return Ok(true);}
    }
    Ok(false)
}
/// Teto de cada reserva do pane a um clique de mod: os 7,5 s do pedido mais os 2 s da limpeza, com folga; a
/// limpeza renova a sua a cada volta ao prompt, também abaixo disto. Vence sozinha: uma tarefa de clique que
/// sumiu sem o `Release` não segura a fila.
pub(crate) const MAX_MODS_HOLD:Duration=Duration::from_secs(10);
pub struct TerminalActor;
impl TerminalActor {
    pub fn spawn(target:TerminalTarget,queue:QueueActor,policy:PolicyClient,options:TerminalOptions,events:broadcast::Sender<RuntimeEvent>,revision:Arc<AtomicU64>)->TerminalHandle {
        let previous=queue.initial_state().operations.keys().filter(|id|id.starts_with("call::terminal:") || id.starts_with("terminal-policy:") || id.starts_with("queue:"))
            .filter_map(|id|id.rsplit(':').next()?.parse::<u64>().ok()).max().unwrap_or(0);
        let sequence=Arc::new(AtomicU64::new(queue.initial_state().next_seq.max(previous.max(queue.initial_state().operations.len() as u64).saturating_add(1))));
        let receipt=ReceiptIndex::new("claude",&target.binding.conversation);
        let (sender,receiver)=mpsc::channel(64); let closed=Arc::new(AtomicBool::new(false)); let anchor=options.anchor.clone();
        let executor=Executor {target,queue:Arc::new(queue),policy,options,events,revision,sequence,receipt,deliverable:false,last_error:None,uncertain:vec![],unprovable:false,loan:None,stall:None,hold:None,hold_checked:false,parked:VecDeque::new(),away:None,clear_watch:None,clear_notice:None,footer_misses:None};
        let task=tokio::spawn(executor.run(receiver,closed.clone()));
        TerminalHandle {sender,closed,task:Arc::new(Mutex::new(Some(task))),stopped:Arc::new(Mutex::new(None)),anchor}
    }
}
impl Executor {
    /// Prazo vencido: o Rust retoma o teclado sem esperar devolução.
    fn loaned(&mut self)->bool {
        if self.loan.as_ref().is_some_and(|(_,deadline,_)|tokio::time::Instant::now()>=*deadline) {
            tracing::warn!(key=%self.target.key,session=%self.target.name,code="keyboard_loan_expired","prazo do teclado emprestado venceu; o Rust retomou o pane");
            self.loan=None;
        }
        self.loan.is_some()
    }
    /// Pane reservado a um clique de mod (`PaneOp::Hold`); vence sozinho no prazo, mesmo sem o `Release`.
    fn held(&mut self)->bool {
        if self.hold.is_some_and(|until|tokio::time::Instant::now()>=until) {self.hold=None; self.hold_checked=false;}
        self.hold.is_some()
    }
    /// O Python pede o teclado por uma operação; fila, trava e estado continuam aqui. O ator é serial:
    /// quando o pedido chega não há digitação em curso.
    fn loan_control(&mut self,id:&str,kind:&str,payload:&Value)->Result<RuntimeReply,RuntimeError> {
        let active=self.loaned();
        if kind=="keyboard_loan" {
            let seconds=payload["seconds"].as_u64().filter(|s|(1..=MAX_LOAN_S).contains(s))
                .filter(|_|payload.as_object().is_some_and(|p|p.len()==1)).ok_or_else(||error("terminal_payload"))?;
            if let Some((loan_id,deadline,request))=self.loan.as_ref().filter(|_|active) {
                // O mesmo pedido repetido (resposta perdida) recebe o mesmo empréstimo, não `keyboard_busy`.
                if request==id {return Ok(reply(id,Disposition::Accepted,json!({"loan_id":loan_id,
                    "seconds":deadline.saturating_duration_since(tokio::time::Instant::now()).as_secs()})))}
                return Ok(reply(id,Disposition::Rejected,json!({"code":"keyboard_busy"})));
            }
            let loan_id=format!("loan:{}:{}",self.target.generation,self.sequence.fetch_add(1,Ordering::Relaxed));
            self.loan=Some((loan_id.clone(),tokio::time::Instant::now()+Duration::from_secs(seconds),id.into()));
            return Ok(reply(id,Disposition::Accepted,json!({"loan_id":loan_id,"seconds":seconds})));
        }
        let loan_id=payload["loan_id"].as_str().filter(|_|payload.as_object().is_some_and(|p|p.len()==1)).ok_or_else(||error("terminal_payload"))?;
        if active && self.loan.as_ref().is_some_and(|(current,_,_)|current==loan_id) {
            self.loan=None;
            return Ok(reply(id,Disposition::Accepted,json!({"returned":true})));
        }
        Ok(reply(id,Disposition::Rejected,json!({"code":"keyboard_loan_expired"})))
    }
    fn cleared(&self,state:&super::queue::State)->bool {
        state.runtime_state["clear_barrier"]["generation"]==self.target.generation
            && state.runtime_state["clear_barrier"]["conversation"]==self.target.binding.conversation
    }
    fn services(&self,root:&str,attempt:&str,text:&str)->Arc<Services> {Arc::new(Services {target:self.target.clone(),queue:self.queue.clone(),policy:self.policy.clone(),root:root.into(),attempt:attempt.into(),text:text.into(),sequence:self.sequence.clone()})}
    async fn action(&self,action:Action)->Result<Value,RuntimeError> {self.services("maintenance","maintenance","").action("queue",action).await}
    async fn snapshot(&self)->Result<Value,RuntimeError> {
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.generation!=self.target.generation{return Err(error("runtime_generation"));}
        Ok(json!({"key":self.target.key,"generation":self.target.generation,"revision":self.revision.load(Ordering::Acquire),
            "view":{"terminal":true,"conversation":self.target.binding.conversation,"deliverable":self.deliverable && !self.cleared(&state),"preserve_binding":state.runtime_state["preserve_binding"],"clear_barrier":state.runtime_state["clear_barrier"],
                "input_stalled":self.stall.as_ref().filter(|s|s.surfaced).map(|s|s.code.clone()).or_else(||self.clear_notice.map(|_|"clear_not_applied".into()))},"channels":{},"error":self.last_error}))
    }
    async fn publish(&self)->Result<(),RuntimeError> {
        self.revision.fetch_add(1,Ordering::AcqRel);
        let data=self.snapshot().await?;
        let _=self.events.send(RuntimeEvent {key:self.target.key.clone(),generation:self.target.generation,revision:self.revision.load(Ordering::Acquire),channel:"snapshot".into(),data}); Ok(())
    }
    async fn enter_error(&mut self,failure:RuntimeError)->Result<(),RuntimeError> {
        if self.last_error.as_deref()==Some(failure.code.as_str()){return Ok(());}
        tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,reason=%failure.message,"entrada terminal entrou em erro");
        self.last_error=Some(failure.code.clone()); self.deliverable=false;
        self.publish().await?;
        let revision=self.revision.fetch_add(1,Ordering::AcqRel)+1;
        let _=self.events.send(RuntimeEvent {key:self.target.key.clone(),generation:self.target.generation,revision,channel:"problem".into(),
            data:json!({"error_code":failure.code,"message":failure.message})}); Ok(())
    }
    async fn clear_maintenance_error(&mut self,code:&str)->Result<(),RuntimeError> {
        if self.last_error.as_deref()==Some(code) {
            self.last_error=None; self.publish().await?;
        }
        Ok(())
    }
    async fn run(mut self,mut receiver:mpsc::Receiver<Message>,closed:Arc<AtomicBool>)->Result<(),RuntimeError> {
        self.action(Action::Recover).await?; self.action(Action::EnsureProjection).await?;
        let recovered=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        // O `/clear` que não virou conversa nova (recusado pela saída da trava) ou que parou antes do
        // Enter não ergue a trava de novo.
        if let Some(clear)=recovered.operations.values().find(|op|op.payload["kind"]=="input" && op.payload["payload"]["text"].as_str().is_some_and(is_clear)
            && match op.status {Status::Accepted|Status::Confirmed=>true,Status::Unknown=>clear_may_have_run(Disposition::Unknown,&op.result["payload"]),_=>false}
            && op.wire_attempts.keys().any(|wire|wire.starts_with(&format!("terminal:{}:",self.target.generation)))) {
            let mut state=recovered.runtime_state.clone(); state["preserve_binding"]=json!(true);
            let old=Some(state["clear_barrier"].clone()).filter(|b|b["operation_id"]==clear.id.as_str());
            let since=old.as_ref().and_then(|b|b["since"].as_f64()).unwrap_or_else(||sample().epoch_s);
            let raised=old.as_ref().and_then(|b|b["raised"].as_f64()).unwrap_or(since);
            state["clear_barrier"]=json!({"generation":self.target.generation,"conversation":self.target.binding.conversation,"operation_id":clear.id,"since":since,"raised":raised});
            self.action(Action::SetRuntimeState {state}).await?;
        }
        if self.cleared(&self.queue.snapshot().await.map_err(|_|error("queue_io"))?) {self.clear_watch=Some(tokio::time::Instant::now()+self.options.clear_wait);}
        let mut timer=tokio::time::interval(self.options.tick.max(Duration::from_millis(1))); timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // Guardadas durante um clique de mod: saem na ordem em que chegaram, assim que ele solta o pane.
            // `unparked`: a mensagem saiu da fila local e não volta para ela (sem laço).
            let (message,unparked)=if !self.parked.is_empty() && !self.held() {(self.parked.pop_front(),true)} else {
                // A reserva que vence sem `Release` solta o que estava guardado na hora, mesmo com o relógio
                // desligado pelo erro de manutenção.
                let release_at=self.hold.filter(|_|!self.parked.is_empty());
                let message=tokio::select! {biased;
                    message=receiver.recv()=>message,
                    _=tokio::time::sleep_until(release_at.unwrap_or_else(tokio::time::Instant::now)),if release_at.is_some()=>continue,
                    _=timer.tick(),if !closed.load(Ordering::Acquire) && (self.last_error.is_none() || self.reconciling() || self.clear_watch.is_some() || self.clear_notice.is_some())=>{
                        if self.clear_notice.is_some_and(|until|tokio::time::Instant::now()>=until) {self.clear_notice=None; self.publish().await?;}
                        // Clique de mod em curso, ou o que ele guardou ainda por sair: a fila não entrega nem
                        // reconcilia no pane antes disso (C6).
                        if self.held() || !self.parked.is_empty() {continue;}
                        // O `/clear` incerto depois do Enter deixa o erro de entrega incerta: a trava ainda vence.
                        if self.last_error.is_some() {self.expire_clear().await; if self.reconciling() {self.reconcile_uncertain().await?;} continue;}
                        let result=async {self.confirm_rows().await?;self.drain_once(None).await?;Ok::<_,RuntimeError>(())}.await;
                        if let Err(failure)=result {self.enter_error(failure).await?;}
                        continue;
                    }
                };
                (message,false)
            };
            // O que escreve no pane (comando e drenagem pedida) espera o clique de mod soltar, e o que chega
            // com algo ainda guardado entra atrás dele (ordem da caixa); operação de mod, fila, retrato,
            // confirmação e parada seguem.
            let wait=!unparked && (self.held() || !self.parked.is_empty());
            let message=match message {
                Some(message@(Message::Command {..}|Message::Drain(_))) if wait=>{self.parked.push_back(message);continue;},
                other=>other,
            };
            match message {
                Some(Message::Command {id,kind,payload,response})=>{
                    let result=if matches!(kind.as_str(),"keyboard_loan"|"keyboard_return") {self.loan_control(&id,&kind,&payload)}
                        else {self.execute(&id,&kind,payload,None).await};
                    let _=response.send(result);},
                Some(Message::Queue {id,action,response})=>{
                    let result=match action {
                        Action::Finish {id,status,result}=>self.native_receipt(&id,status,result).await,
                        Action::Claim {..}|Action::SetDelivered {value:false,..}|Action::BumpAttempts {..}|Action::Reconcile {..}|Action::ReplaceRows {..}|Action::Prepare {..}|Action::BeginDispatch {..}|Action::MarkWriting {..}|Action::BindDispatch {..}|Action::Recover|Action::Confirm {..}|Action::ConfirmOccurrence {..}|Action::ConfirmLegacy {..}|Action::SetRuntimeState {..}|Action::LateRpcResolution {..}=>Err(error("terminal_queue_action")),
                        action=>self.queue.exec(self.target.generation,&id,sample(),action).await.map_err(|_|error("queue_io")),
                    };
                    if result.is_ok(){self.publish().await?;} let _=response.send(result);
                },
                Some(Message::Pane {op,start_by,response})=>{
                    // Esperou na caixa além do ponto de partida: não age (C1). O soltar é a exceção: uma limpeza
                    // atrasada ainda solta a fila, senão ela ficaria guardada até o fim da reserva.
                    let late=op!=PaneOp::Release && std::time::Instant::now()>=start_by;
                    let result=if late {Err(error("mods_deadline"))} else {self.pane_op(op).await};
                    let _=response.send(result);},
                Some(Message::Snapshot(response))=>{let _=response.send(self.snapshot().await);},
                Some(Message::Drain(response))=>{let result=self.drain_once(None).await;
                    // Durante o empréstimo nada foi relido: o erro de manutenção continua valendo.
                    if let Err(failure)=&result {self.enter_error(failure.clone()).await?;}
                    else if result.as_ref().is_ok_and(|v|v["keyboard_loan"]!=true) {self.clear_maintenance_error("terminal_facts").await?;}
                    let _=response.send(result);},
                Some(Message::Confirm(response))=>{let result=self.confirm_rows().await;
                    if let Err(failure)=&result {self.enter_error(failure.clone()).await?;}else{self.clear_maintenance_error("receipt_scan").await?;}
                    let _=response.send(result);},
                Some(Message::Stop(response))=>{
                    closed.store(true,Ordering::Release);
                    self.refuse_parked();
                    let queue=Arc::try_unwrap(self.queue).map_err(|_|error("terminal_lease_inflight"))?;
                    let result=queue.shutdown().await.map_err(|_|error("queue_stop")); let _=response.send(result.clone()); return result;
                },
                None=>{self.refuse_parked(); let queue=Arc::try_unwrap(self.queue).map_err(|_|error("terminal_lease_inflight"))?;return queue.shutdown().await.map_err(|_|error("queue_stop"));}
            }
        }
    }
    /// Parada com algo guardado pelo clique de mod: quem esperava ouve `runtime_stopping`, como quem chama
    /// um ator que já está parando, e não a caixa fechada.
    fn refuse_parked(&mut self) {
        for message in self.parked.drain(..) {
            match message {
                Message::Command {response,..}=>{let _=response.send(Err(error("runtime_stopping")));},
                Message::Drain(response)=>{let _=response.send(Err(error("runtime_stopping")));},
                _=>{},
            }
        }
    }
    /// Quem tem o teclado do pane, pela tela dos mods, logo antes de digitar uma entrada: com o foco num
    /// painel ou na faixa de um mod (a limpeza de um clique que não o devolveu ao prompt), o `Enter` da
    /// entrada apertaria um botão do mod, e o `Escape` que o tiraria de lá age no Claude. `None` libera a
    /// escrita; um diálogo sem prompt também, porque a escrita já o adia como `overlay`.
    /// A tela dos mods, se o foco está num painel ou na faixa reconhecidos e o retrato do que a pessoa mexe.
    /// `Ok(None)` com o tamanho ilegível (a guarda não trava a entrega por isso); `Err` com a tela ilegível.
    async fn mods_view(&self)->Result<Option<(crate::mods::screen::Screen,bool)>,&'static str> {
        let driver=TerminalDriver::new(self.target.binding.clone(),Arc::new(NoFacts),self.options.io.clone(),self.options.limits.clone());
        // O tamanho só delimita a leitura: sem ele (formato ilegível no multiplexador) a guarda não trava
        // toda entrega por um detalhe de leitura, e a escrita segue como antes.
        let formats=match driver.mods_formats().await {
            Ok(formats)=>formats,
            Err(failure)=>{
                if crate::warn_limit::allow(Some(self.target.key.as_str()),"terminal_focus_formats") {
                    tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"tamanho do pane ilegível; a entrada segue sem conferir o foco dos mods");
                }
                return Ok(None);
            }
        };
        // A tela que não se lê também não seria escrita: a entrada espera, com o motivo.
        let ansi=driver.mods_screen().await.map_err(|failure|failure.code)?;
        Ok(Some(self.mods_read(&ansi,formats)))
    }
    /// A leitura da tela dos mods num tamanho já conhecido: a tela e se o foco está num mod.
    fn mods_read(&self,ansi:&str,formats:input::PaneFormats)->(crate::mods::screen::Screen,bool) {
        let (columns,rows)=(usize::from(formats.columns),usize::from(formats.rows));
        let anchor=self.options.anchor.lock().unwrap().clone();
        let screen=crate::mods::screen::read_screen(ansi,columns,rows,&[],anchor.as_deref());
        // Só o que a leitura reconhece como mod conta: um realce do próprio Claude Code (seleção, menu)
        // acima do prompt não segura a mensagem da pessoa. Painel: com borda ou caixa e região. Faixa: o
        // inverso dentro da região que a leitura achou, a inteira pela âncora do mod, a recolhida e a
        // encolhida pela própria linha.
        let grid=crate::mods::screen::parse_ansi(ansi,columns,rows);
        let away=match screen.focus {
            Some("pane")=>screen.placement.is_some() && screen.body.is_some(),
            Some("band")=>screen.band.as_ref().is_some_and(|band|
                (band.rows.0..band.rows.1).any(|r|grid.get(r).is_some_and(|line|line[band.lo.min(line.len())..band.hi.min(line.len())].iter().any(|c|c.inverse)))),
            _=>false,
        };
        (screen,away)
    }
    /// Quem tem o teclado do pane, pela tela dos mods, logo antes de digitar a entrada `row`: com o foco num
    /// painel ou na faixa de um mod, o `Enter` da entrada apertaria um botão do mod, e a escrita adia
    /// (`mods_focus`). Um diálogo sem prompt libera, porque a escrita já o adia como `overlay`.
    ///
    /// A saída da fila: o foco num mod por mais que `focus_return` desde a primeira vez que a linha o viu (a
    /// limpeza de um clique que desistiu, ou quem levou o foco lá e saiu) é devolvido ao prompt, e a entrada
    /// escreve. A linha tenta `FOCUS_RETURN_TRIES` devoluções; depois, e com só a faixa na tela, só registra e
    /// espera a pessoa. A contagem é da linha: a seguinte começa do zero.
    async fn focus_guard(&mut self,row:&str)->Option<&'static str> {
        let band_only=match self.mods_view().await {
            Err(code)=>return Some(code),
            Ok(Some((screen,true)))=>screen.placement.is_none(),
            Ok(_)=>{self.away=None; return None;},
        };
        let away=Some("mods_focus");
        let now=tokio::time::Instant::now();
        if self.away.as_ref().is_none_or(|state|state.row!=row) {
            self.away=Some(Away {row:row.into(),since:now,next:now+self.options.focus_return,tries:0});
        }
        let Some(state)=self.away.as_mut() else {return away};
        if now<state.next || state.tries>=FOCUS_RETURN_TRIES {return away;}
        state.tries+=1;
        let (since,tries)=(state.since,state.tries);
        // Só com a faixa na tela o anel do `ctrl+x tab` gira dentro dela e nunca chega ao prompt (medição (i)):
        // nenhuma tecla, só o registro.
        if band_only {
            state.tries=FOCUS_RETURN_TRIES;
            tracing::warn!(key=%self.target.key,session=%self.target.name,code="mods_focus_band_only",
                "o foco segue na faixa de um mod sem painel aberto; o ctrl+x tab não o devolve, e a entrada espera a pessoa");
            return away;
        }
        if self.return_focus().await {
            tracing::info!(key=%self.target.key,session=%self.target.name,code="mods_focus_returned",waited_s=now.duration_since(since).as_secs(),
                "o foco seguia num mod com uma entrada esperando; o executor o devolveu ao prompt");
            self.away=None;
            return None;
        }
        // A segunda tentativa espera o dobro do prazo.
        if let Some(state)=self.away.as_mut() {state.next=tokio::time::Instant::now()+self.options.focus_return*2;}
        tracing::warn!(key=%self.target.key,session=%self.target.name,code="mods_focus_return_failed",tries,
            "o foco não voltou ao prompt; a entrada espera");
        away
    }
    /// Volta o teclado ao prompt pelo `ctrl+x tab`, a tecla medida para isso (o `Escape` age no Claude, e no
    /// psmux nem devolve o foco), lendo a tela antes de cada tecla. Para com um diálogo ou a pesquisa na tela,
    /// onde qualquer tecla mexe neles. `true` quando o foco saiu do mod.
    ///
    /// Como o anel do clique: o pane é conferido e o tamanho lido uma vez, e cada passo é a tecla e uma
    /// leitura, repetida até a tela mudar, até o dobro do `settle` da entrada (300 ms). Na VM
    /// cada passo custava 0,5 s com a conferência e o tamanho relidos e uma pausa fixa.
    async fn return_focus(&self)->bool {
        let driver=TerminalDriver::new(self.target.binding.clone(),Arc::new(NoFacts),self.options.io.clone(),self.options.limits.clone());
        let Ok(formats)=driver.mods_formats().await else {return false};
        let driver=driver.pane_checked();
        let Ok(mut ansi)=driver.mods_screen().await else {return false};
        for _ in 0..FOCUS_RETURN_KEYS {
            match self.mods_read(&ansi,formats) {
                (screen,true) if !screen.dialog && !screen.survey=>{},
                (_,false)=>return true,
                _=>return false,
            }
            if driver.mods_keys(&["C-x","Tab"]).await.is_err() {return false;}
            // A tecla aparece em 50 a 60 ms; um botão da faixa sem desenho não muda a tela, e o passo segue no
            // teto.
            let until=tokio::time::Instant::now()+self.options.limits.settle*2;
            loop {
                let Ok(now)=driver.mods_screen().await else {return false};
                let changed=now!=ansi;
                ansi=now;
                if changed || tokio::time::Instant::now()>=until {break;}
                tokio::time::sleep(FOCUS_STEP_POLL).await;
            }
        }
        !self.mods_read(&ansi,formats).1
    }
    /// Clique, roda, tecla da reserva, leitura, tamanho e reserva do pane para os mods. Com o teclado
    /// emprestado ao Python (administração digitando no pane), recusa: duas mãos no mesmo pane erram o alvo.
    async fn pane_op(&mut self,op:PaneOp)->Result<PaneReply,RuntimeError> {
        // Soltar vale sempre: é a limpeza do clique.
        if op==PaneOp::Release {self.hold=None; self.hold_checked=false; return Ok(PaneReply::Done);}
        if self.loaned() {return Err(error("keyboard_loan"));}
        if let PaneOp::Hold {millis}=op {
            // Cada reserva, também a renovação da limpeza, confere o pane de novo uma vez: uma sessão do
            // multiplexador trocada por fora com o mesmo nome não vale por todas as renovações.
            self.hold_checked=false;
            self.hold=Some(tokio::time::Instant::now()+Duration::from_millis(millis).min(MAX_MODS_HOLD));
            return Ok(PaneReply::Done);
        }
        // Dentro de uma reserva o pane é conferido uma vez: no psmux cada conferência é mais um processo, e
        // a reserva por teclado com uma dúzia de botões na faixa estourava o prazo do pedido.
        let checked=self.held() && self.hold_checked;
        let mut driver=TerminalDriver::new(self.target.binding.clone(),Arc::new(NoFacts),self.options.io.clone(),self.options.limits.clone());
        if checked {driver=driver.pane_checked();}
        let reply=self.pane_effect(&driver,op).await;
        if reply.is_ok() && self.hold.is_some() {self.hold_checked=true;}
        reply
    }
    async fn pane_effect(&self,driver:&TerminalDriver,op:PaneOp)->Result<PaneReply,RuntimeError> {
        let failed=|failure:input::IoFailure|error(failure.code);
        Ok(match op {
            PaneOp::Formats=>PaneReply::Formats(driver.mods_formats().await.map_err(failed)?),
            PaneOp::Clients=>PaneReply::Clients(driver.mods_clients().await.map_err(failed)?),
            PaneOp::Screen=>PaneReply::Screen(driver.mods_screen().await.map_err(failed)?),
            PaneOp::Mouse {row,col}=>{driver.mouse(row,col).await.map_err(failed)?; PaneReply::Done},
            PaneOp::Wheel {row,col,down}=>{driver.wheel(row,col,down).await.map_err(failed)?; PaneReply::Done},
            PaneOp::Keys(keys)=>{let keys:Vec<&str>=keys.iter().map(String::as_str).collect(); driver.mods_keys(&keys).await.map_err(failed)?; PaneReply::Done},
            PaneOp::Resize {columns,rows}=>{driver.resize(columns,rows).await.map_err(failed)?; PaneReply::Done},
            PaneOp::Hold {..}|PaneOp::Release=>PaneReply::Done,
        })
    }
    async fn execute(&mut self,id:&str,kind:&str,payload:Value,entry:Option<String>)->Result<RuntimeReply,RuntimeError> {
        if id.is_empty() || id.starts_with("call::") || id.starts_with("terminal-") {return Err(error("operation_id"));}
        validate(kind,&payload)?;
        let requested=json!({"operation_id":id,"kind":kind,"payload":payload});
        let mut original=requested.clone();
        if kind=="input" && !payload["text"].as_str().unwrap_or("").trim_start().starts_with('/') {
            original["payload"]["_terminal_generation"]=json!(self.target.generation);
        }
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.generation!=self.target.generation{return Err(error("runtime_generation"));}
        if let Some(old)=state.operations.get(id) {
            if old.payload!=original && !(old.payload["payload"].get("_terminal_generation").is_none() && old.payload==requested) {return Err(error("operation_payload"));}
            if let Ok(stored)=serde_json::from_value::<RuntimeReply>(old.result.clone()){return Ok(stored);}
            if matches!(old.status,Status::Prepared|Status::Deferred) {
                return Ok(reply(id,Disposition::Deferred,json!({"code":"prepared_before_dispatch","queued":old.entry_id.is_some(),"cleanup":"not_needed"})));
            }
            return Ok(reply(id,Disposition::Unknown,json!({"code":"recovered_operation"})));
        }
        if self.cleared(&state) && !self.expire_clear().await {return Err(error("runtime_clear_barrier"));}
        let text=payload["text"].as_str().unwrap_or("");
        let prompt=kind=="input"; let slash=prompt && text.trim_start().starts_with('/');
        let row_id=if prompt && !slash {Some(entry.unwrap_or_else(||id.into()))}else{None};
        let prepared=self.action(Action::Prepare {id:id.into(),payload:original,entry_id:row_id.clone()}).await?;
        if let Some(disposition)=match prepared["status"].as_str() {
            Some("accepted"|"confirmed")=>Some(Disposition::Accepted),Some("rejected")=>Some(Disposition::Rejected),_=>None,
        } {
            return Ok(serde_json::from_value(prepared["result"].clone()).unwrap_or_else(|_|reply(id,disposition,Value::Null)));
        }
        if let Some(row)=&row_id {
            if !state.rows.iter().any(|r|r["id"]==*row) {
                self.action(Action::Append {text:text.into(),delivered:false,ts:None,pre_transcript:payload["pre_transcript"]==true,entry_id:Some(row.clone())}).await?;
            }
        }
        if prompt && !is_clear(text) && state.terminal_write_blocked(&self.target.binding.conversation) {
            let result=reply(id,Disposition::Deferred,json!({"code":"terminal_write_barrier","queued":row_id.is_some(),"cleanup":"not_needed"}));
            self.action(Action::Finish {id:id.into(),status:Status::Deferred,result:serde_json::to_value(&result).unwrap()}).await?;
            return Ok(result);
        }
        if self.loaned() {
            // O Python está digitando no pane: a entrada com linha na fila espera e sai depois da
            // devolução; comando e controle, sem fila para esperar, são recusados com o código.
            let (disposition,status)=if row_id.is_some() {(Disposition::Deferred,Status::Deferred)} else {(Disposition::Rejected,Status::Rejected)};
            let result=reply(id,disposition,json!({"code":"keyboard_loan","queued":row_id.is_some(),"cleanup":"not_needed"}));
            self.action(Action::Finish {id:id.into(),status,result:serde_json::to_value(&result).unwrap()}).await?;
            return Ok(result);
        }
        let root=row_id.as_deref().unwrap_or(id);
        // Antes da escrita: o transcript novo do `/clear` nasce logo depois do Enter.
        let dispatched_at=sample().epoch_s;
        let services=self.services(root,id,text);
        let mut driver=TerminalDriver::new(self.target.binding.clone(),services.clone(),self.options.io.clone(),self.options.limits.clone());
        if self.footer_misses.as_ref().is_some_and(|(row,misses)|row==root && *misses>=FOCUS_RETURN_TRIES) {driver=driver.footer_kept();}
        let facts=if prompt {Some(services.facts(&self.target.binding).await)}else{None};
        let current=facts.as_ref().is_none_or(|result|result.as_ref().is_ok_and(|facts|facts.binding==self.target.binding));
        if let Some(facts)=&facts {self.deliverable=facts.as_ref().is_ok_and(|facts|current && facts.ready && facts.idle && !facts.open_question);}
        let mut result=if !current {reply(id,Disposition::Deferred,json!({"code":"terminal_facts","cleanup":"not_needed"}))}
            else if prompt && !slash && !facts.as_ref().is_some_and(|result|result.as_ref().is_ok_and(|f|f.ready && !f.open_question || f.native.is_some())) {reply(id,Disposition::Deferred,json!({"queued":true,"cleanup":"not_needed"}))}
            else {
                if row_id.is_some() {
                    let cursor=self.receipt.capture(&self.target.transcript).map_err(|_|error("receipt_cursor"))?;
                    self.action(Action::BindDispatch {id:id.into(),cursor:serde_json::to_value(cursor).unwrap()}).await?;
                }
                // A entrada só escreve depois do `writing` do escritor; antes disso, cair não a deixa incerta.
                self.action(Action::BeginDispatch {id:id.into(),wire_id:format!("terminal:{}:{id}",self.target.generation),staged:kind=="input"}).await?;
                let publication=format!("{}:{}:{id}",self.target.key,self.target.generation);
                let held=payload["request_id"].as_str().filter(|s|s.starts_with("perm:")||s.starts_with("ask:"));
                let plugin=if matches!(kind,"select"|"answer_questions") {
                    if let Some(request)=held {
                        Some(services.call("terminal_plugin_control",RequestId::String(request.into()),json!({"binding":self.target.binding,"operation_id":root,"control":kind,"payload":payload,"publication_id":publication,"generation":self.target.generation})).await)
                    }else{None}
                }else{None};
                match plugin {
                    Some(Ok(value)) if value["disposition"]!="unavailable"=> {
                        let disposition=match value["disposition"].as_str(){Some("accepted")=>Disposition::Accepted,Some("rejected")=>Disposition::Rejected,_=>Disposition::Unknown}; reply(id,disposition,value)
                    },
                    Some(Err(_))=>reply(id,Disposition::Unknown,json!({"code":"plugin_control_uncertain"})),
                    _=>delivery(id,match kind {
                        // Foco fora do prompt: adia sem escrever, como o `overlay`; a linha volta à fila e
                        // o próximo tique tenta de novo.
                        // Só quando a entrega vai apertar tecla: o modo `User` e a nativa não passam pelo teclado,
                        // e a guarda só custaria leituras e atraso.
                        "input"=>match if facts.as_ref().is_some_and(|f|f.as_ref().is_ok_and(|f|!input::presses_keys(f,text))) {None} else {self.focus_guard(root).await} {
                            Some(code)=>DeliveryResult {disposition:input::Disposition::Deferred,stage:input::DeliveryStage::Composer,cleanup:input::Cleanup::NotNeeded,
                                native:false,message_id:None,code:code.into(),draft:None},
                            None=>driver.prompt(text,&publication).await,
                        },
                        "steer"|"steer_queue"=>driver.steer().await,
                        "key"|"navigation_key"=>driver.key(payload["key"].as_str().unwrap(),false).await,
                        "interactive_key"=>driver.key(payload["key"].as_str().unwrap(),true).await,
                        "terminal_input"=>driver.text(text).await,
                        "select"=>driver.select(payload["option"].as_u64().unwrap() as usize,payload["require_cursor"].as_bool().unwrap_or(true)).await,
                        "answer_questions"=>driver.answer(&serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()).unwrap()).await,
                        "submit_selected"=>driver.submit_selected().await,
                        "interrupt"=>driver.interrupt(payload["clear"].as_bool().unwrap_or(false)).await,
                        _=>unreachable!(),
                    })
                }
            };
        // Só o desfecho: o texto do rascunho nunca sai do terminal; antes de
        // qualquer `?`, para a falha do diário não calar o aviso.
        if let Some(draft)=result.payload["draft"].as_str().filter(|draft|*draft!="returned") {
            tracing::warn!(key=%self.target.key,session=%self.target.name,draft,code=%result.payload["code"].as_str().unwrap_or(""),
                reason="o rascunho do dono não voltou igual ao composer; se o terminal marca › stashed, ele volta com Ctrl+S","rascunho do terminal");
        }
        if result.payload["code"]=="footer_focus" {
            let misses=self.footer_misses.as_ref().filter(|(row,_)|row==root).map_or(0,|(_,n)|*n)+1;
            if misses==FOCUS_RETURN_TRIES {tracing::warn!(key=%self.target.key,session=%self.target.name,code="footer_focus_kept","o Esc não devolveu o foco do rodapé ao composer; a entrada espera a pessoa");}
            self.footer_misses=Some((root.to_string(),misses));
        } else if self.footer_misses.as_ref().is_some_and(|(row,_)|row==root)
            && !matches!(result.payload["stage"].as_str(),Some("validate"|"identity"|"ready")) {self.footer_misses=None;}
        let clear_raised=slash && is_clear(text) && clear_may_have_run(result.disposition,&result.payload);
        if clear_raised {
            let mut state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?.runtime_state;
            state["preserve_binding"]=json!(true);
            state["clear_barrier"]=json!({"generation":self.target.generation,"conversation":self.target.binding.conversation,"operation_id":id,"since":dispatched_at,"raised":sample().epoch_s});
            self.action(Action::SetRuntimeState {state}).await?;
            result.payload["preserve_binding"]=json!(true);
            self.clear_watch=Some(tokio::time::Instant::now()+self.options.clear_wait); self.clear_notice=None;
        }
        self.action(Action::Finish {id:id.into(),status:status(result.disposition),result:serde_json::to_value(&result).unwrap()}).await?;
        let repeated=row_id.as_deref().is_some_and(|row|self.track_stall(row,&result));
        if matches!(result.disposition,Disposition::Deferred|Disposition::Rejected) && !repeated {
            tracing::info!(key=%self.target.key,session=%self.target.name,code=%result.payload["code"].as_str().unwrap_or("terminal_not_executed"),
                reason="operação adiada ou recusada; resultado conservado no diário",stage=%result.payload["stage"].as_str().unwrap_or("plugin"),"resultado da entrada terminal");
        }
        if result.disposition==Disposition::Unknown {
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%result.payload["code"].as_str().unwrap_or("plugin_control_uncertain"),
                reason="a entrega não foi comprovada",stage=%result.payload["stage"].as_str().unwrap_or("plugin"),"entrega terminal incerta");
            if row_id.is_some() {self.uncertain.push(id.into());} else if !clear_raised {self.unprovable=true;}
            self.enter_error(error("terminal_delivery_unknown")).await?;
        }else{self.last_error=None; self.uncertain.clear(); self.unprovable=false; self.publish().await?;}
        Ok(result)
    }
    /// Conta a série de recusas sem escrita; devolve se esta repete o código anterior (sem log novo).
    fn track_stall(&mut self,row:&str,result:&RuntimeReply)->bool {
        let Some(code)=stalled_code(result) else {self.stall=None; return false;};
        let now=tokio::time::Instant::now();
        // A série é da linha: outra linha começa do zero, sem herdar a espera de quem saiu da fila.
        let stall=match self.stall.take().filter(|stall|stall.row==row) {
            // Outro código na mesma série não zera o relógio: alternar entre dois nunca apareceria.
            // `wait` zerado é o composer que esvaziou: a espera recomeça do tique.
            Some(mut stall)=>{let repeated=stall.code==code; stall.code=code.into(); stall.wait=(stall.wait*2).max(self.options.tick).min(MAX_STALL_WAIT); (stall,repeated)}
            None=>(Stall {row:row.into(),code:code.into(),since:now,wait:self.options.tick,next:now,surfaced:false},false),
        };
        let (stall,repeated)=stall;
        let stall=self.stall.insert(Stall {next:now+stall.wait,..stall});
        if !stall.surfaced && now.duration_since(stall.since)>=self.options.stall_notice {
            stall.surfaced=true;
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%stall.code,
                waited_s=now.duration_since(stall.since).as_secs(),"entrada terminal parada: o terminal recusa a entrega sem escrever");
        }
        repeated
    }
    async fn drain_once(&mut self,entry:Option<String>)->Result<Value,RuntimeError> {
        if self.loaned() {return Ok(json!({"drained":0,"keyboard_loan":true}));}
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.terminal_write_blocked(&self.target.binding.conversation) {return Ok(json!({"drained":0}));}
        if self.cleared(&state) && !self.expire_clear().await {return Ok(json!({"drained":0,"preserve_binding":true}));}
        if !state.rows.iter().any(|r|r["delivered"]==false && entry.as_ref().is_none_or(|id|r["id"]==*id)) {
            self.away=None;
            if self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
            return Ok(json!({"drained":0}));
        }
        // A linha parada saiu da fila (apagada ou entregue): a próxima não espera por ela.
        if self.stall.as_ref().is_some_and(|s|!state.rows.iter().any(|r|r["delivered"]==false && r["id"]==s.row.as_str()))
            && self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
        let services=self.services("maintenance","maintenance","");
        let facts=services.facts(&self.target.binding).await.map_err(|_|error("terminal_facts"))?;
        self.deliverable=facts.binding==self.target.binding && facts.ready && facts.idle && !facts.open_question;
        if !self.deliverable || facts.binding!=self.target.binding {
            // Terminal ocupado ou com pergunta é espera normal: o motivo antigo sai da tela.
            if self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
            return Ok(json!({"drained":0}));
        }
        // A espera crescente só segura a escrita: os fatos (e o `deliverable`) seguem frescos, e o
        // composer que esvaziou (o dono enviou o rascunho) libera a tentativa na hora.
        if self.stall.as_ref().is_some_and(|s|tokio::time::Instant::now()<s.next) {
            let driver=TerminalDriver::new(self.target.binding.clone(),services.clone(),self.options.io.clone(),self.options.limits.clone());
            if !driver.composer_free().await {return Ok(json!({"drained":0,"stalled":true}));}
            if let Some(stall)=self.stall.as_mut() {stall.wait=Duration::ZERO;}
        }
        let rows=self.action(Action::Claim {min_ts:self.target.created,limit:Some(1),entry_id:entry}).await?;
        let Some(row)=rows.as_array().and_then(|r|r.first())else{return Ok(json!({"drained":0}));};
        let row_id=row["id"].as_str().ok_or_else(||error("queue_row"))?.to_string();
        let attempt=format!("queue:{}:{}",row_id,self.sequence.fetch_add(1,Ordering::Relaxed));
        let outcome=self.execute(&attempt,"input",json!({"text":row["text"],"pre_transcript":row["pre_transcript"]==true}),Some(row_id)).await?;
        Ok(json!({"drained":1,"reply":outcome}))
    }
    async fn confirm_rows(&mut self)->Result<Value,RuntimeError> {
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if !state.rows.iter().any(|r|r["delivered"]==true && r["confirmed"]!=true){return Ok(json!({"confirmed":0}));}
        self.receipt.scan(&self.target.transcript).map_err(|_|error("receipt_scan"))?;
        let mut count=0;
        for operation in state.operations.values().filter(|op|op.entry_id.is_some() && matches!(op.status,Status::Accepted|Status::Unknown|Status::Dispatching)) {
            let current=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
            let Some(row)=current.rows.iter().find(|r|r["id"].as_str()==operation.entry_id.as_deref() && r["confirmed"]!=true) else {continue;};
            let Ok(cursor)=serde_json::from_value::<DispatchCursor>(operation.dispatch_cursor.clone())else{continue;};
            if let Some(proof)=self.receipt.match_after(&self.target.transcript,&cursor,row,&current.used_occurrences).map_err(|_|error("receipt_scan"))? {
                if self.action(Action::ConfirmOccurrence {id:operation.id.clone(),proof}).await?==true {count+=1;}
            }
        }
        // Depois das despachadas: uma linha que prova a entrega nova não pode ser gasta por uma legada.
        let current=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        let legacy=if self.cleared(&current) {Vec::new()} else {super::queue::legacy_rows(&current)};
        for row in legacy {
            let used=self.queue.snapshot().await.map_err(|_|error("queue_io"))?.used_occurrences;
            let (Some(entry_id),Some((occurrence,normalized_text)))=(row["id"].as_str().map(str::to_owned),self.receipt.match_legacy(&row,&used)) else {continue;};
            // Falha numa legada não desfaz as despachadas já confirmadas: fica para a próxima rodada.
            match self.action(Action::ConfirmLegacy {entry_id,occurrence,normalized_text}).await {
                Ok(accepted)=>if accepted==true {count+=1;},
                Err(failure)=>tracing::warn!(key=%self.target.key,code=%failure.code,"confirmação de entrada legada falhou; segue na próxima rodada"),
            }
        }
        if count>0{self.publish().await?;}Ok(json!({"confirmed":count}))
    }
    fn reconciling(&self)->bool {
        self.last_error.as_deref()==Some("terminal_delivery_unknown") && !self.unprovable && !self.uncertain.is_empty()
    }
    /// Só lê o transcript: a entrega incerta que aparece lá é confirmada uma vez e o erro sai; sem
    /// prova, o erro fica e nada é digitado.
    async fn reconcile_uncertain(&mut self)->Result<(),RuntimeError> {
        let state=match self.confirm_rows().await {
            Ok(_)=>self.queue.snapshot().await.map_err(|_|error("queue_io")),
            Err(failure)=>Err(failure),
        };
        let state=match state {
            Ok(state)=>state,
            Err(failure)=>{
                if crate::warn_limit::allow(Some(self.target.key.as_str()),"terminal_reconcile_failed") {
                    tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"conferência da entrega incerta falhou; o erro continua");
                }
                return Ok(());
            }
        };
        if !self.uncertain.iter().all(|id|state.operations.get(id).is_some_and(|op|op.status==Status::Confirmed)) {return Ok(());}
        tracing::info!(key=%self.target.key,session=%self.target.name,code="terminal_delivery_proved","entrega incerta comprovada pelo transcript");
        self.uncertain.clear();
        self.clear_maintenance_error("terminal_delivery_unknown").await
    }
    /// A saída da trava do `/clear`: passado o prazo com a sessão parada e sem conversa nova (nem nos
    /// fatos do Python nem no disco), o `/clear` não foi aplicado. Com o Claude trabalhando ele espera na
    /// fila do próprio Claude Code e roda no fim do turno. A trava sai, a operação fica recusada (a reabertura não a
    /// ergue de novo), as linhas da fila seguem e a vista avisa. Nada é reenviado. `true` = soltou.
    async fn expire_clear(&mut self)->bool {
        let now=tokio::time::Instant::now();
        if self.clear_watch.is_none_or(|at|now<at) {return false;}
        let state=match self.queue.snapshot().await {Ok(state)=>state,Err(_)=>return false};
        if !self.cleared(&state) {self.clear_watch=None; return false;}
        let changed=match self.services("maintenance","maintenance","").facts(&self.target.binding).await {
            Ok(facts)=>facts.binding!=self.target.binding || !facts.idle,
            Err(failure)=>{
                if crate::warn_limit::allow(Some(self.target.key.as_str()),"clear_barrier_facts") {
                    tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.0,"fatos indisponíveis; a trava do /clear fica e confere de novo");
                }
                self.clear_watch=Some(now+self.options.clear_wait); return false;
            }
        };
        // Relido depois dos fatos: a liberação grava o estado inteiro.
        let state=match self.queue.snapshot().await {Ok(state)=>state,Err(_)=>return false};
        if !self.cleared(&state) {self.clear_watch=None; return false;}
        let barrier=state.runtime_state["clear_barrier"].clone();
        let since=barrier["since"].as_f64().unwrap_or(0.0);
        let raised=barrier["raised"].as_f64().unwrap_or(since);
        // Fora do ator: a pasta pode ter milhares de transcripts.
        let transcript=self.target.transcript.clone();
        let scan=if sample().epoch_s-raised<CLEAR_DISK_TRUST_S {
            tokio::task::spawn_blocking(move||clear_on_disk(&transcript,since)).await.unwrap_or_else(|failure|{
                tracing::warn!(key=%self.target.key,code="clear_barrier_disk_panic",reason=%failure,"varredura do transcript caiu; a trava do /clear fica");
                Err(std::io::Error::other("clear_on_disk"))
            })
        } else {Ok(false)};
        let on_disk=scan.unwrap_or_else(|failure|{
            if crate::warn_limit::allow(Some(self.target.key.as_str()),"clear_barrier_disk") {
                tracing::warn!(key=%self.target.key,session=%self.target.name,code="clear_barrier_disk",kind=?failure.kind(),"transcript ilegível; a trava do /clear fica");
            }
            true
        });
        // Ocupada, ou a conversa nova existe e só falta o Python reabrir: a troca do vínculo solta a trava.
        if changed || on_disk {
            self.clear_watch=Some(now+self.options.clear_wait); return false;
        }
        // Primeiro a operação: se a trava não sair do diário, a reabertura não a ergue de novo por ela.
        let id=barrier["operation_id"].as_str().unwrap_or_default().to_string();
        if let Some(op)=state.operations.get(&id) {
            let result=reply(&id,Disposition::Rejected,json!({"code":"clear_not_applied","stage":op.result["payload"]["stage"],"cleanup":"not_needed"}));
            if let Err(failure)=self.action(Action::Finish {id:id.clone(),status:Status::Rejected,result:serde_json::to_value(result).unwrap()}).await {
                tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"/clear não aplicado não foi marcado; a trava fica e confere de novo");
                self.clear_watch=Some(now+self.options.clear_wait); return false;
            }
        }
        let mut runtime=match self.queue.snapshot().await {Ok(state)=>state.runtime_state,Err(_)=>{self.clear_watch=Some(now+self.options.clear_wait); return false;}};
        if let Some(fields)=runtime.as_object_mut() {fields.remove("clear_barrier"); fields.remove("preserve_binding");}
        if let Err(failure)=self.action(Action::SetRuntimeState {state:runtime}).await {
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"trava do /clear não saiu do diário; tenta de novo");
            self.clear_watch=Some(now+self.options.clear_wait); return false;
        }
        tracing::warn!(key=%self.target.key,session=%self.target.name,code="clear_not_applied",operation=%id,
            "o /clear não virou conversa nova no prazo; a trava saiu e a fila segue, sem reenviar o /clear");
        self.clear_watch=None; self.clear_notice=Some(now+CLEAR_NOTICE);
        // A incerteza era a do `/clear`, que agora se sabe não aplicado: a fila volta a drenar.
        if self.last_error.as_deref()==Some("terminal_delivery_unknown") && self.uncertain.is_empty() && !self.unprovable {self.last_error=None;}
        if let Err(failure)=self.publish().await {tracing::warn!(key=%self.target.key,code=%failure.code,"vista do /clear não aplicado não publicou");}
        true
    }
    async fn native_receipt(&self,id:&str,receipt_status:Status,result:Value)->Result<Value,RuntimeError> {
        let native_status=result["payload"]["native_status"].as_str().ok_or_else(||error("native_receipt"))?;
        let disposition=match native_status {"delivered"|"released"|""=>Disposition::Accepted,"rejected"|"refused"=>Disposition::Rejected,_=>Disposition::Unknown};
        if result.as_object().is_none_or(|o|o.len()!=3) || result["operation_id"]!=id
            || result["disposition"]!=serde_json::to_value(disposition).unwrap()
            || result["payload"].as_object().is_none_or(|o|o.len()!=1)
            || serde_json::to_value(receipt_status).unwrap()!=serde_json::to_value(status(disposition)).unwrap() {return Err(error("native_receipt"));}
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if !state.rows.iter().any(|row|row["id"]==id) {return Err(error("native_receipt"));}
        let root=state.operations.get(id).filter(|op|op.payload["kind"]=="input" && op.entry_id.as_deref()==Some(id));
        let attempts:Vec<_>=state.operations.values().filter(|op|op.entry_id.as_deref()==Some(id) && op.payload["kind"]=="input"
            && (op.payload["payload"]["_terminal_generation"].as_u64()==Some(self.target.generation)
                || op.payload["payload"].get("_terminal_generation").is_none() && op.wire_attempts.keys().any(|wire|wire.starts_with(&format!("terminal:{}:",self.target.generation))))
            && op.result["payload"]["native"]==true && op.result["payload"]["message_id"].is_string()).collect();
        let source=attempts.last().ok_or_else(||error("native_receipt"))?;
        let mut payload=source.result["payload"].clone(); payload["native_status"]=json!(native_status);
        for attempt in &attempts {
            if attempt.status==Status::Confirmed {continue;}
            let resolved=reply(&attempt.id,disposition,payload.clone());
            self.action(Action::Finish {id:attempt.id.clone(),status:receipt_status,result:serde_json::to_value(resolved).unwrap()}).await?;
        }
        let resolved=reply(id,disposition,payload);
        if root.is_some_and(|root|root.status!=Status::Confirmed) {
            self.action(Action::Finish {id:id.into(),status:receipt_status,result:serde_json::to_value(&resolved).unwrap()}).await?;
        }
        Ok(serde_json::to_value(resolved).unwrap())
    }
}
fn is_clear(text:&str)->bool {text.split_whitespace().next()==Some("/clear")}
fn validate(kind:&str,payload:&Value)->Result<(),RuntimeError> {
    let fields:&[&str]=match kind {
        "input"=>&["text","pre_transcript"],"steer"=>&[],"key"|"navigation_key"|"interactive_key"=>&["key"],"terminal_input"=>&["text"],
        "select"=>&["option","require_cursor","request_id"],"answer_questions"=>&["answers","request_id"],"interrupt"=>&["clear"],"submit_selected"=>&[],"steer_queue"=>&["entry_id"],_=>return Err(error("terminal_control")),
    };
    if !payload.as_object().is_some_and(|p|p.keys().all(|k|fields.contains(&k.as_str()))){return Err(error("terminal_payload"));}
    let valid=match kind {
        "input"=>payload["text"].as_str().is_some_and(valid_text) && payload.get("pre_transcript").is_none_or(Value::is_boolean),
        "terminal_input"=>payload["text"].as_str().is_some_and(safe_characters),
        "key"|"navigation_key"|"interactive_key"=>payload["key"].is_string(),
        "select"=>payload["option"].as_u64().is_some_and(|v|v>0 && v<=100) && payload.get("require_cursor").is_none_or(Value::is_boolean) && payload.get("request_id").is_none_or(Value::is_string),
        "answer_questions"=>serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()).is_ok_and(|answers|!answers.is_empty() && answers.iter().all(|a|match a.kind {
            AnswerKind::Option=>!a.indices.is_empty() && a.indices.iter().all(|i|*i<100) && (a.multi||a.indices.len()==1) && !a.labels.is_empty(),
            AnswerKind::Text=>a.type_index.is_some_and(|i|i<100) && a.value.as_ref().is_some_and(|v|valid_text(v)&&!v.contains('\n')),
            AnswerKind::Chat=>a.chat_index.is_some_and(|i|i<100),
        })) && payload.get("request_id").is_none_or(Value::is_string),
        "interrupt"=>payload.get("clear").is_none_or(Value::is_boolean),"steer_queue"=>payload.get("entry_id").is_none_or(Value::is_string),_=>true,
    };
    if !valid{return Err(error("terminal_payload"));}Ok(())
}
