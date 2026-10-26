//! Executor terminal serial; diário, posse e confirmação são os mesmos da fila.
use super::{actor::PolicyClient,protocol::{ClockSample,Disposition,RuntimeCommand,RuntimeError,RuntimeEvent,RuntimeReply},queue::{Action,QueueActor,Status},receipt::{DispatchCursor,ReceiptIndex}};
use crate::terminal_input::{self as input,TerminalBinding,TerminalDriver,TerminalIo,TerminalServices,InputFacts,InputLimits,PluginRequest,PluginReply,ServiceFuture,ServiceError,DeliveryResult,QuestionAnswer,AnswerKind};
use crate::mods::click::{Pane,PaneFuture,PaneOp,PaneReply};
use crate::interactions::{ExpectedInteraction,InteractionBinding,InteractionError,InteractionStore,runtime_error};
use serde_json::{Value,json};
use std::{collections::VecDeque,path::PathBuf,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}},time::{Duration,SystemTime,UNIX_EPOCH}};
use tokio::sync::{Mutex,mpsc,oneshot,broadcast};
use super::terminal_status::{self, StatusSender};

#[derive(Clone)]
pub struct TerminalTarget {
    pub key:String,pub generation:u64,pub name:String,pub binding:TerminalBinding,
    pub lease_path:PathBuf,pub state_path:PathBuf,pub projection_dir:PathBuf,pub transcript:PathBuf,pub created:f64,
    /// A chave do token da ponte que o processo recebeu no lançamento (`plugin_key` do vínculo).
    pub plugin_key:Option<String>,
    pub metadata:crate::terminal_facts::TerminalMetadata,
}
/// A âncora da faixa dos mods (`mods::tree::anchor`, o começo do primeiro texto dela): o elo do `Mods` a
/// escreve a cada `/ui` do plugin, e o executor a lê para reconhecer na tela a faixa inteira focada.
pub type ModsAnchor=Arc<std::sync::Mutex<Option<String>>>;
/// `stall_notice`: entrega adiada sem escrita por mais que isso aparece na vista e no log. `focus_return`:
/// com uma entrada esperando e o foco num painel ou na faixa de um mod há mais que isso, sem clique em curso,
/// o executor devolve o teclado ao prompt e escreve. `clear_wait`: quanto a trava do `/clear` espera a
/// conversa nova antes de conferir se ele foi aplicado. `give_up_after`: quanto a entrega incerta espera a
/// prova antes de ser desistida sozinha (com o Claude parado).
pub struct TerminalOptions { pub lifecycle:Option<std::sync::Weak<super::gateway::RuntimeRegistry>>,pub permissions:Option<Arc<std::sync::Mutex<crate::state::permission::PermissionMemory>>>,pub interactions:Option<Arc<InteractionStore>>,pub plugin:Option<Arc<crate::plugin::Store>>,pub mode_facts:Option<crate::terminal_facts::ModeReader>,pub terms:Option<Arc<crate::term::Terms>>,pub io:Arc<dyn TerminalIo>,pub limits:InputLimits,pub tick:Duration,pub stall_notice:Duration,pub anchor:ModsAnchor,pub focus_return:Duration,pub clear_wait:Duration,pub give_up_after:Duration,pub status:Option<StatusSender>,pub turn_start_wait:Duration,pub start_grace:Duration }
impl Default for TerminalOptions {
    fn default()->Self {Self {lifecycle:None,permissions:None,interactions:None,plugin:None,mode_facts:None,terms:None,io:Arc::new(input::ProcessIo::default()),limits:InputLimits::default(),tick:Duration::from_secs(1),stall_notice:Duration::from_secs(30),
        anchor:ModsAnchor::default(),focus_return:FOCUS_RETURN,clear_wait:CLEAR_APPLY_WAIT,give_up_after:GIVE_UP_AFTER,status:None,turn_start_wait:TURN_START_WAIT,start_grace:START_GRACE}}
}
/// Quanto uma linha espera, desde a primeira vez que viu o foco num mod, antes de o executor devolver o
/// teclado ao prompt. Cobre quem acabou de levar o foco a um painel no terminal, sem deixar a mensagem do
/// app parada até alguém mexer no terminal; nenhum clique do app está em curso, porque com a reserva do pane
/// a entrada nem chega aqui.
const FOCUS_RETURN:Duration=Duration::from_secs(10);
/// Depois de uma entrega comum, quanto a fila espera o Claude aparecer ocupado antes de levar a próxima: a prova
/// pode chegar antes de o registro nativo dizer `busy`, e um turno curto demais nunca aparece ocupado num tique.
const TURN_START_WAIT:Duration=Duration::from_secs(3);
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
/// O Claude só põe o bypass no Shift+Tab de quem nasceu com ele.
/// argv ilegível é erro com código, nunca "sem bypass" (que reabriria a sessão à toa).
fn bypass_in_cycle(agent:u32)->Result<bool,RuntimeError> {
    use crate::list::procs::ProcessView;
    let argv=crate::list::procs::SystemProcs::default().argv_checked(i64::from(agent)).map_err(|_|error("lifecycle_process_read"))?;
    Ok(argv_has_bypass(&argv))
}
fn argv_has_bypass(argv:&[String])->bool {
    argv.iter().any(|a|a=="--dangerously-skip-permissions" || a=="--allow-dangerously-skip-permissions" || a=="--permission-mode=bypassPermissions")
        || argv.windows(2).any(|pair|pair[0]=="--permission-mode" && pair[1]=="bypassPermissions")
}
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
    Lifecycle {id:String,action:Action,response:oneshot::Sender<Result<Value,RuntimeError>>},
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
    pub(super) async fn lifecycle(&self,id:String,action:Action)->Result<Value,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(error("runtime_stopping"));}
        if !matches!(&action,Action::Prepare {..}|Action::BeginDispatch {..}|Action::Finish {..}) {return Err(error("lifecycle_action"));}
        let (response,receive)=oneshot::channel();
        self.sender.send(Message::Lifecycle {id,action,response}).await.map_err(|_|error("runtime_closed"))?;
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
pub(crate) struct NoFacts;
impl TerminalServices for NoFacts {
    fn permission_guard<'a>(&'a self,_:&'a TerminalBinding)->ServiceFuture<'a,()> {Box::pin(async {Err(ServiceError("terminal_permission_guard_unavailable"))})}
    fn facts<'a>(&'a self,_:&'a TerminalBinding)->ServiceFuture<'a,InputFacts> {Box::pin(async {Err(ServiceError("mods_no_facts"))})}
    fn publish<'a>(&'a self,_:&'a TerminalBinding,_:PluginRequest)->ServiceFuture<'a,PluginReply> {Box::pin(async {Err(ServiceError("mods_no_publish"))})}
}

struct Services {
    lifecycle:Option<std::sync::Weak<super::gateway::RuntimeRegistry>>,
    target:TerminalTarget,queue:Arc<QueueActor>,local:Arc<crate::terminal_facts::LocalTerminalFacts>,terms:Option<Arc<crate::term::Terms>>,root:String,attempt:String,text:String,sequence:Arc<AtomicU64>,life_proved:AtomicBool,
}
impl Services {
    async fn action(&self,label:&str,action:Action)->Result<Value,RuntimeError> {self.action_held(label,action,false).await}
    async fn action_held(&self,label:&str,action:Action,hold:bool)->Result<Value,RuntimeError> {
        let seq=self.sequence.fetch_add(1,Ordering::Relaxed);
        self.queue.exec_held(self.target.generation,&format!("terminal:{label}:{seq}"),sample(),action,hold).await.map_err(|_|error("queue_io"))
    }
}
impl TerminalServices for Services {
    fn permission_guard<'a>(&'a self,binding:&'a TerminalBinding)->ServiceFuture<'a,()> {Box::pin(async move {
        let terms=self.terms.as_ref().ok_or(ServiceError("terminal_permission_guard_unavailable"))?;
        if terms.is_active(&self.target.name) {return Err(ServiceError("terminal_panel_active"));}
        self.local.check(&self.target,binding).await?;
        let driver=TerminalDriver::new(binding.clone(),Arc::new(NoFacts),self.local.io_for_guard(),InputLimits::default());
        driver.facts_screen().await.map_err(|error|ServiceError(error.code))?;
        self.local.check(&self.target,binding).await?;
        if terms.is_active(&self.target.name) {return Err(ServiceError("terminal_panel_active"));}
        Ok(())
    })}
    fn facts<'a>(&'a self,binding:&'a TerminalBinding)->ServiceFuture<'a,InputFacts> {Box::pin(async move {
        // Uma entrega, um `Services`: a vida provada numa leitura vale para as seguintes desta entrega,
        // cada uma reconferindo-a no fim; nada passa para a próxima entrega.
        let read=if self.life_proved.load(Ordering::Acquire) {self.local.reread(&self.target,binding,&self.text).await} else {self.local.read(&self.target,binding,&self.text).await};
        self.life_proved.store(read.is_ok(),Ordering::Release);
        let mut facts=read.map_err(|failure| {
            if crate::warn_limit::allow(Some(&self.target.key),"terminal_local_facts") {
                tracing::warn!(key=%self.target.key,session=%self.target.name,code=failure.0,"fatos locais indisponíveis; a entrada espera sem escrever");
            }
            failure
        })?;
        if let Some(native)=&mut facts.native {
            // Recado `[de: …]`: a caixa do Rust é o endereço de resposta, e o recibo de recado retido ou
            // recusado volta a esta entrada. Fora dele (manutenção, clique de mod) não há recado.
            let route=crate::terminal_input::message_prefix(&self.text).and(self.lifecycle.as_ref()).and_then(std::sync::Weak::upgrade)
                .map(|registry|registry.native_route(&self.target.key,&self.root));
            native.message_id=Some(match route {
                Some((message_id,origin))=>{native.origin=origin;message_id},
                None=>crate::terminal_facts::message_id(&self.target.key,&self.root),
            });
        }
        Ok(facts)
    })}
    fn writing<'a>(&'a self)->ServiceFuture<'a,()> {Box::pin(async move {
        self.action("writing",Action::MarkWriting {id:self.attempt.clone(),wire_id:format!("terminal:{}:{}",self.target.generation,self.attempt)}).await
            .map(|_|()).map_err(|failure|{
                tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,"diário não gravou o início da escrita; a entrada espera");
                ServiceError("write_journal")
            })
    })}
    fn publish<'a>(&'a self,binding:&'a TerminalBinding,request:PluginRequest)->ServiceFuture<'a,PluginReply> {Box::pin(async move {
        self.local.publish(&self.target,binding,request).await
    })}
    fn facts_check_pane(&self)->bool {true}
    fn life_check<'a>(&'a self,binding:&'a TerminalBinding)->ServiceFuture<'a,()> {Box::pin(async move {
        self.local.check_agent(&self.target,binding).await.map(|_|())
    })}
    fn abandon_publication<'a>(&'a self,publication_id:&'a str)->ServiceFuture<'a,()> {Box::pin(async move {
        self.local.abandon_publication(&self.target,publication_id)
    })}
    fn transcript_mark<'a>(&'a self)->ServiceFuture<'a,Option<u64>> {Box::pin(async move {
        let path=self.target.transcript.clone();
        tokio::task::spawn_blocking(move||std::fs::metadata(path).ok().map(|m|m.len())).await.map_err(|_|ServiceError("terminal_transcript_unavailable"))
    })}
    fn transcript_enqueued<'a>(&'a self,mark:u64,text:&'a str)->ServiceFuture<'a,bool> {Box::pin(async move {
        let (path,text)=(self.target.transcript.clone(),text.to_owned());
        tokio::task::spawn_blocking(move||crate::terminal_input::enqueued_since(&path,mark,&text)).await.map_err(|_|ServiceError("terminal_transcript_unavailable"))
    })}
    fn prompt_started<'a>(&'a self,since:f64,text:&'a str)->ServiceFuture<'a,bool> {Box::pin(async move {
        let (dir,conversation,text)=(self.target.metadata.config_dir.clone(),self.target.binding.conversation.clone(),text.to_owned());
        tokio::task::spawn_blocking(move||crate::terminal_input::prompt_started_in(&dir,&conversation,since,&text)).await.map_err(|_|ServiceError("terminal_prompt_mark_unavailable"))
    })}
}

struct Executor {
    batch_blocked:bool,
    target:TerminalTarget,queue:Arc<QueueActor>,local:Arc<crate::terminal_facts::LocalTerminalFacts>,options:TerminalOptions,
    events:broadcast::Sender<RuntimeEvent>,revision:Arc<AtomicU64>,sequence:Arc<AtomicU64>,receipt:ReceiptIndex,deliverable:bool,last_error:Option<String>,
    /// Entradas com linha na fila cuja entrega ficou incerta nesta vida do ator: o transcript ainda
    /// pode prová-las. Incerteza sem linha (tecla, seleção) não tem prova e só sai na reabertura.
    uncertain:Vec<String>,unprovable:bool,
    /// Troca de modo sem prova do resultado: segura só a reabertura até uma leitura confirmar o modo.
    mode_unproved:bool,
    /// Ciclo de modos já sondado nesta vida (conversa): leituras e sondas seguintes o devolvem sem teclas.
    modes_cache:Option<Value>,
    /// Dona da trava de escrita e quando ela foi vista pela primeira vez nesta vida do ator (o prazo de desistir).
    uncertain_since:Option<(String,tokio::time::Instant)>,
    /// Teclado emprestado ao Python (administração que digita no pane): id, prazo e o pedido que o abriu.
    loan:Option<(String,tokio::time::Instant,String)>,
    /// Linha da fila que o terminal recusa sem escrever (composer ocupado, tela ilegível): espera
    /// crescente entre as tentativas e, passado o `stall_notice`, o motivo na vista.
    stall:Option<Stall>,
    /// Pane reservado a um clique de mod (`PaneOp::Hold`) até este instante.
    hold:Option<tokio::time::Instant>,
    /// A identidade do pane já foi conferida nesta reserva: as operações seguintes do clique não a refazem.
    hold_checked:bool,
    /// O plugin já não devolveu o foco nesta reserva: as voltas seguintes da limpeza vão direto ao anel.
    focus_refused:bool,
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
    reconciliation:Option<tokio::task::JoinHandle<Result<Value,RuntimeError>>>,
    /// Depois de uma entrega aceita, a confirmação segue o crescimento do transcript em vez de esperar o
    /// tique; a prova é a mesma (`confirm_rows`). Prazo para não vigiar uma linha que nunca chega.
    confirm_until:Option<tokio::time::Instant>,
    confirm_seen:Option<u64>,
    /// Desde quando nada prova a conversa (sem registro nativo do agente nem `/pull` do plugin); passado o
    /// `stall_notice`, a vista mostra `no_safe_delivery`.
    unproved_since:Option<tokio::time::Instant>,
    no_safe_delivery:bool,
    /// Quando a reabertura deste incidente foi pedida. Reabertura que falhou é pedida de novo depois do
    /// intervalo do registro, senão a fila ficava parada para sempre com o aviso prometendo reabrir.
    heal_requested:Option<tokio::time::Instant>,
    /// Entrega comum aceita com o Claude parado: até este instante (ou até ele aparecer ocupado) a fila
    /// trata a sessão como trabalhando, para a próxima não cair no mesmo turno.
    turn_wait:Option<tokio::time::Instant>,
    diag:crate::diag::DiagClient,
    /// Se esta vida já provou a conversa: até lá, com o agente novo, sem a prova é o Claude subindo.
    conversation_proved:bool,
}
/// Leitura do tamanho do transcript enquanto uma entrega espera confirmação: só `stat`.
const CONFIRM_POLL:Duration=Duration::from_millis(50);
const CONFIRM_WINDOW:Duration=Duration::from_secs(30);
struct Stall {row:String,code:String,since:tokio::time::Instant,wait:Duration,next:tokio::time::Instant,surfaced:bool}
/// Teto da espera entre tentativas: o composer que esvazia é visto na hora, e o resto (tela
/// ilegível) não deve atrasar a mensagem muito além disso.
const MAX_STALL_WAIT:Duration=Duration::from_secs(8);
/// Id de operação que o pedido pode usar: os prefixos `call::` e `terminal-` são do próprio ator.
fn valid_operation_id(id:&str)->bool {!id.is_empty() && !id.starts_with("call::") && !id.starts_with("terminal-")}
/// Leituras da marca "ctrl+x ctrl+s to send now" depois de digitar a entrada orientada.
/// Quanto o Orientar espera a dica "ctrl+x ctrl+s to send now": o Claude Code só a desenha depois dos hooks de
/// UserPromptSubmit da conta. Parado, a entrega já abre o turno e a dica não vem: espera menos.
const STEER_HINT_WAIT:Duration=Duration::from_secs(5);
const STEER_HINT_WAIT_IDLE:Duration=Duration::from_secs(2);
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
/// Vida nova sem a conversa provada ainda (o Claude subindo): espera assim, sem erro, até este prazo.
const START_GRACE:Duration=Duration::from_secs(60);
/// O `give_up_after` padrão: a prova chega em segundos; minutos sem ela, com o Claude parado, é perda.
const GIVE_UP_AFTER:Duration=Duration::from_secs(180);
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
        if path==transcript || !path.file_name().is_some_and(|n|hangar_workspace::worktrees::is_conversation_file(&n.to_string_lossy())) {continue;}
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
        let local=Arc::new(crate::terminal_facts::LocalTerminalFacts::new(options.io.clone(),options.plugin.clone(),options.mode_facts.clone()));
        let status=options.status.clone();
        if let Some(sender)=&status { sender.send_replace(Default::default()); }
        let executor=Executor {batch_blocked:false,target,queue:Arc::new(queue),local,options,events,revision,sequence,receipt,deliverable:false,last_error:None,uncertain:vec![],unprovable:false,mode_unproved:false,modes_cache:None,uncertain_since:None,loan:None,stall:None,hold:None,hold_checked:false,focus_refused:false,parked:VecDeque::new(),away:None,clear_watch:None,clear_notice:None,footer_misses:None,reconciliation:None,confirm_until:None,confirm_seen:None,
            unproved_since:None,no_safe_delivery:false,heal_requested:None,turn_wait:None,diag:policy.diag(),conversation_proved:false};
        let run=executor.run(receiver,closed.clone());
        let task=tokio::spawn(async move {
            let mut guard=terminal_status::PanicMark(status);
            let result=run.await;
            if let Some(sender)=guard.0.take() && let Err(failure)=&result {
                terminal_status::publish_problem(&sender,&failure.code,&failure.message);
            }
            result
        });
        TerminalHandle {sender,closed,task:Arc::new(Mutex::new(Some(task))),stopped:Arc::new(Mutex::new(None)),anchor}
    }
}
impl Executor {
    async fn reconcile_life(&mut self)->Result<(),RuntimeError> {
        if self.reconciliation.as_ref().is_some_and(|task|task.is_finished()) {
            let result=self.reconciliation.take().unwrap().await.map_err(|_|error("lifecycle_panic"))?;
            result?;
            if self.last_error.as_deref().is_some_and(|code|code.starts_with("lifecycle_") || code=="state_lifecycle_unbound") {self.last_error=None;self.publish().await?;}
        }
        if self.reconciliation.is_none() {
            let registry=self.options.lifecycle.as_ref().and_then(std::sync::Weak::upgrade).ok_or_else(||error("state_lifecycle_unbound"))?;
            let target=self.target.clone();
            self.reconciliation=Some(tokio::spawn(async move {registry.reconcile_claude(&target.name,&target.key,&target.binding.conversation,target.generation).await}));
        }
        Ok(())
    }
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
    fn services(&self,root:&str,attempt:&str,text:&str)->Arc<Services> {Arc::new(Services {lifecycle:self.options.lifecycle.clone(),target:self.target.clone(),queue:self.queue.clone(),local:self.local.clone(),terms:self.options.terms.clone(),root:root.into(),attempt:attempt.into(),text:text.into(),sequence:self.sequence.clone(),life_proved:AtomicBool::new(false)})}
    async fn action(&self,action:Action)->Result<Value,RuntimeError> {self.services("maintenance","maintenance","").action("queue",action).await}
    async fn snapshot(&self)->Result<Value,RuntimeError> {
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.generation!=self.target.generation{return Err(error("runtime_generation"));}
        Ok(json!({"key":self.target.key,"generation":self.target.generation,"revision":self.revision.load(Ordering::Acquire),
            "view":{"terminal":true,"working_revision":self.working_revision(),"conversation":self.target.binding.conversation,"deliverable":self.deliverable && !self.cleared(&state),"preserve_binding":state.runtime_state["preserve_binding"],"clear_barrier":state.runtime_state["clear_barrier"],
                "input_stalled":self.stall.as_ref().filter(|s|s.surfaced).map(|s|s.code.clone()).or_else(||self.clear_notice.map(|_|"clear_not_applied".into()))
                    .or_else(||state.terminal_write_blocked(&self.target.binding.conversation).then(||"delivery_unproved".to_owned()))
                    .or_else(||self.no_safe_delivery.then(||"no_safe_delivery".to_owned()))
                    .or_else(||self.batch_blocked.then(||super::queue::BATCH_TOO_LARGE.to_owned()))},"channels":{},"error":self.last_error}))
    }
    async fn publish(&self)->Result<(),RuntimeError> {
        self.revision.fetch_add(1,Ordering::AcqRel);
        let data=self.snapshot().await?;
        if let Some(sender)=&self.options.status { terminal_status::publish_snapshot(sender,&data); }
        let _=self.events.send(RuntimeEvent {key:self.target.key.clone(),generation:self.target.generation,revision:self.revision.load(Ordering::Acquire),channel:"snapshot".into(),data}); Ok(())
    }
    async fn enter_error(&mut self,failure:RuntimeError)->Result<(),RuntimeError> {
        if self.last_error.as_deref()==Some(failure.code.as_str()){return Ok(());}
        tracing::warn!(key=%self.target.key,session=%self.target.name,code=%failure.code,reason=%failure.message,"entrada terminal entrou em erro");
        self.last_error=Some(failure.code.clone()); self.deliverable=false;
        self.publish().await?;
        if let Some(sender)=&self.options.status { terminal_status::publish_problem(sender,&failure.code,&failure.message); }
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
        let binding=json!({"key":self.target.key,"conversation":self.target.binding.conversation,"generation":self.target.generation});
        for operation in recovered.operations.values() {
            if operation.result["payload"]["interaction_binding"]==binding {
                if let Some(revision)=operation.result["payload"]["consumed_source_revision"].as_str() {
                    crate::state::ask::consume_revision(&self.target.transcript,revision).map_err(|_|error("askq_consumption_unavailable"))?;
                }
            }
        }
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
        if self.cleared(&*self.queue.snapshot().await.map_err(|_|error("queue_io"))?) {self.clear_watch=Some(tokio::time::Instant::now()+self.options.clear_wait);}
        if let Some(sender)=&self.options.status { terminal_status::publish_snapshot(sender,&self.snapshot().await?); }
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
                    _=timer.tick(),if !closed.load(Ordering::Acquire) && (self.last_error.is_none() || self.last_error.as_deref()==Some("terminal_facts") || self.reconciling() || self.clear_watch.is_some() || self.clear_notice.is_some() || self.options.lifecycle.is_some())=>{
                        if self.clear_notice.is_some_and(|until|tokio::time::Instant::now()>=until) {self.clear_notice=None; self.publish().await?;}
                        // Clique de mod em curso, ou o que ele guardou ainda por sair: a fila não entrega nem
                        // reconcilia no pane antes disso (C6).
                        if self.held() || !self.parked.is_empty() {continue;}
                        // Sem ciclo de vida ligado a reconciliação só falharia; a trava do `/clear` sai pelo `expire_clear` da drenagem.
                        if self.options.lifecycle.is_some() && let Err(failure)=self.reconcile_life().await {self.enter_error(failure).await?;}
                        self.watch_proof().await?;
                        self.expire_uncertain().await;
                        if self.last_error.is_none() {self.expire_unproved().await;}
                        // O `/clear` incerto depois do Enter deixa o erro de entrega incerta: a trava ainda vence.
                        if self.last_error.as_deref()==Some("terminal_facts") {
                            match self.drain_once(None,false).await {
                                Ok(value) if value["keyboard_loan"]!=true && value["preserve_binding"]!=true=>self.clear_maintenance_error("terminal_facts").await?,
                                Ok(_)=>{},Err(failure)=>self.enter_error(failure).await?,
                            }
                            continue;
                        }
                        if self.last_error.is_some() {self.expire_clear().await; if self.reconciling() {self.reconcile_uncertain().await?;} continue;}
                        let result=async {self.confirm_rows().await?;self.drain_once(None,false).await?;Ok::<_,RuntimeError>(())}.await;
                        if let Err(failure)=result {self.enter_error(failure).await?;}
                        continue;
                    }
                    _=tokio::time::sleep(CONFIRM_POLL),if self.confirm_until.is_some() && !closed.load(Ordering::Acquire) && self.last_error.is_none()=>{
                        if self.held() || !self.parked.is_empty() {continue;}
                        if self.confirm_until.is_some_and(|until|tokio::time::Instant::now()>=until) {self.confirm_until=None; continue;}
                        let len=std::fs::metadata(&self.target.transcript).ok().map(|m|m.len());
                        if len==self.confirm_seen {continue;}
                        self.confirm_seen=len;
                        if let Err(failure)=self.confirm_rows().await {self.enter_error(failure).await?; continue;}
                        if self.queue.snapshot().await.is_ok_and(|state|!state.rows.iter().any(|r|r["delivered"]==true && r["confirmed"]!=true)) {self.confirm_until=None;}
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
                        // Fora do `execute`: o Orientar entrega pela drenagem, que já chama o `execute`.
                        else if kind=="steer_entry" {self.steer_entry(&id,&payload).await}
                        else if kind=="steer_first" {self.steer_first(&id).await}
                        else {self.execute(&id,&kind,payload,None,false).await};
                    let _=response.send(result);},
                Some(Message::Queue {id,action,response})=>{
                    let retry_batch=matches!(&action,Action::Append {delivered:false,..}|Action::Edit {..}|Action::Cancel {..}|Action::Remove {..});
                    let result=match action {
                        Action::Finish {id,status,result}=>self.native_receipt(&id,status,result).await,
                        Action::ClaimBatch {..}|Action::Claim {..}|Action::SetDelivered {value:false,..}|Action::BumpAttempts {..}|Action::Reconcile {..}|Action::ReplaceRows {..}|Action::Prepare {..}|Action::BeginDispatch {..}|Action::MarkWriting {..}|Action::BindDispatch {..}|Action::BindDispatchIfPrepared {..}|Action::Recover|Action::Confirm {..}|Action::ConfirmOccurrence {..}|Action::ConfirmLegacy {..}|Action::SetRuntimeState {..}|Action::LateRpcResolution {..}=>Err(error("terminal_queue_action")),
                        action@Action::GiveUp {..}=>{
                            let result=self.queue.exec(self.target.generation,&id,sample(),action).await.map_err(|_|error("queue_io"));
                            if result.as_ref().is_ok_and(|given|*given==json!(true)) {self.release_uncertain().await;}
                            result
                        },
                        action=>self.queue.exec(self.target.generation,&id,sample(),action).await.map_err(|_|error("queue_io")),
                    };
                    if result.is_ok(){if retry_batch {self.batch_blocked=false;}self.publish().await?;} let _=response.send(result);
                },
                Some(Message::Lifecycle {id,action,response})=> {
                    let result=self.queue.exec(self.target.generation,&id,sample(),action).await.map_err(|_|error("queue_io"));
                    let _=response.send(result);
                },
                Some(Message::Pane {op,start_by,response})=>{
                    // Esperou na caixa além do ponto de partida: não age (C1). O soltar é a exceção: uma limpeza
                    // atrasada ainda solta a fila, senão ela ficaria guardada até o fim da reserva.
                    let late=op!=PaneOp::Release && std::time::Instant::now()>=start_by;
                    let result=if late {Err(error("mods_deadline"))} else {self.pane_op(op).await};
                    let _=response.send(result);},
                Some(Message::Snapshot(response))=>{let _=response.send(self.snapshot().await);},
                Some(Message::Drain(response))=>{let result=self.drain_once(None,false).await;
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
        // A tela que não se lê também não seria escrita: a entrada espera, com o motivo. O pane acabou de
        // ser conferido com o tamanho, como no `return_focus`; a tecla que vier depois confere de novo.
        let ansi=driver.pane_checked().mods_screen().await.map_err(|failure|failure.code)?;
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
    /// escreve. Com `plugin` vivo, quem devolve é ele, sem andar pelo anel; sem ele, ou se o foco não voltou,
    /// o `ctrl+x tab`. A linha tenta `FOCUS_RETURN_TRIES` devoluções; depois, e com só a faixa na tela sem o
    /// plugin, só registra e espera a pessoa. A contagem é da linha: a seguinte começa do zero.
    async fn focus_guard(&mut self,row:&str,plugin:bool)->Option<&'static str> {
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
        if plugin && self.plugin_return(row).await && self.focus_back().await {
            tracing::info!(key=%self.target.key,session=%self.target.name,code="mods_focus_returned",by="plugin",waited_s=now.duration_since(since).as_secs(),
                "o foco seguia num mod com uma entrada esperando; o plugin o devolveu ao prompt");
            self.away=None;
            return None;
        }
        // Só com a faixa na tela o anel do `ctrl+x tab` gira dentro dela e nunca chega ao prompt (medição (i)):
        // nenhuma tecla, só o registro.
        if band_only {
            if let Some(state)=self.away.as_mut() {state.tries=FOCUS_RETURN_TRIES;}
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
    /// Pede ao plugin que devolva o teclado ao prompt (`PluginMode::Focus`): ele o faz pelo `prompt.fill`, que
    /// tira o foco da faixa ou do painel sem trocar a aba na frente. `true` só com o aviso dele; o plugin que
    /// não conhece o pedido responde `not_written`, e quem chamou segue pelo `ctrl+x tab`.
    async fn plugin_return(&self,root:&str)->bool {
        let id=format!("{}:{}:focus:{}",self.target.key,self.target.generation,self.sequence.fetch_add(1,Ordering::Relaxed));
        let request=PluginRequest {id:id.clone(),text:String::new(),mode:input::PluginMode::Focus};
        let services=self.services(root,&id,"");
        let result=async {
            services.action("focus-prepare",Action::Prepare {id:id.clone(),payload:json!({"kind":"focus"}),entry_id:None}).await?;
            services.action("focus-dispatch",Action::BeginDispatch {id:id.clone(),wire_id:format!("terminal:{}:{id}",self.target.generation),staged:true}).await?;
            services.writing().await.map_err(|failure|error(failure.0))?;
            let reply=services.publish(&self.target.binding,request).await;
            let status=match &reply {Ok(PluginReply::Filled|PluginReply::Accepted)=>Status::Accepted,Ok(PluginReply::Unavailable|PluginReply::NotWritten)=>Status::Deferred,_=>Status::Unknown};
            services.action("focus-finish",Action::Finish {id:id.clone(),status,result:reply.as_ref().map(|reply|json!(reply)).unwrap_or_else(|failure|json!({"error_code":failure.0}))}).await?;
            reply.map_err(|failure|error(failure.0))
        }.await;
        match result {
            Ok(PluginReply::Filled)=>true,
            reply=>{
                tracing::debug!(key=%self.target.key,session=%self.target.name,reply=?reply,"o plugin não devolveu o foco; segue o ctrl+x tab");
                false
            }
        }
    }
    /// Depois do aviso do plugin, a tela confirma que o foco saiu do mod; o desenho vem logo depois do aviso.
    async fn focus_back(&self)->bool {
        let until=tokio::time::Instant::now()+self.options.limits.settle*2;
        loop {
            match self.mods_view().await {
                Ok(Some((_,true)))=>{},
                Ok(_)=>return true,
                Err(code)=>{tracing::debug!(key=%self.target.key,session=%self.target.name,code,"tela ilegível depois do aviso do plugin; segue o ctrl+x tab"); return false;},
            }
            if tokio::time::Instant::now()>=until {
                tracing::debug!(key=%self.target.key,session=%self.target.name,"o plugin avisou, mas a tela segue num mod; segue o ctrl+x tab");
                return false;
            }
            tokio::time::sleep(FOCUS_STEP_POLL).await;
        }
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
        if op==PaneOp::Release {self.hold=None; self.hold_checked=false; self.focus_refused=false; return Ok(PaneReply::Done);}
        if self.loaned() {return Err(error("keyboard_loan"));}
        if op==PaneOp::ReturnFocus {
            // Uma tentativa por reserva e só com o plugin vivo: sem isso, cada volta da limpeza pagaria o
            // diário e a espera do aviso antes do anel, dentro do prazo curto dela.
            if self.focus_refused {return Err(error("plugin_focus"));}
            let live=self.services("mods-click","mods-click","").facts(&self.target.binding).await.is_ok_and(|facts|facts.plugin_live);
            if live && self.plugin_return("mods-click").await {return Ok(PaneReply::Done);}
            if !live {tracing::debug!(key=%self.target.key,session=%self.target.name,"sem plugin vivo; a limpeza do clique segue o ctrl+x tab");}
            self.focus_refused=self.held();
            return Err(error("plugin_focus"));
        }
        if let PaneOp::Hold {millis}=op {
            // Reserva nova (a anterior venceu sem `Release`): o plugin ganha outra tentativa.
            if !self.held() {self.focus_refused=false;}
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
            // Atendido no `pane_op`, antes do multiplexador.
            PaneOp::ReturnFocus=>PaneReply::Done,
        })
    }
    fn expected(&self,payload:&Value)->Result<ExpectedInteraction,RuntimeError> {
        let supplied=payload["interaction_id"].as_str().filter(|id|!id.is_empty()).ok_or_else(||runtime_error(InteractionError::Missing))?;
        let store=self.options.interactions.as_ref().ok_or_else(||runtime_error(InteractionError::Unavailable))?;
        store.expected(&InteractionBinding {key:self.target.key.clone(),conversation:self.target.binding.conversation.clone(),generation:self.target.generation},Some(supplied)).map_err(runtime_error)
    }

    async fn plugin_control(&self,kind:&str,payload:&Value,expected:&ExpectedInteraction,publication:&str,services:&Services)->DeliveryResult {
        let result=|disposition,code:&str|DeliveryResult {disposition,stage:input::DeliveryStage::Answer,cleanup:input::Cleanup::NotNeeded,native:false,message_id:None,code:code.into(),draft:None,consumed:false};
        let question=match self.local.plugin_question(&self.target,expected).await {Ok(question)=>question,Err(error)=>return result(input::Disposition::Rejected,error.0)};
        let body=if kind=="select" && question.id.starts_with("perm:") {
            match payload["option"].as_u64() {Some(option @ (1|2))=>json!({"permitir":option==1}),_=>return result(input::Disposition::Rejected,"terminal_permission_option")}
        } else if matches!(kind,"answer_questions"|"answer_chat") && expected.target.kind==crate::interactions::InteractionKind::Question {
            match input::plugin_answer_body(&question.questions,&payload["answers"]) {Ok(body)=>body,Err(error)=>return result(input::Disposition::Rejected,error.0)}
        } else {return result(input::Disposition::Rejected,InteractionError::Changed.code());};
        let Some(store)=&self.options.plugin else {return result(input::Disposition::Rejected,InteractionError::Unavailable.code());};
        if services.writing().await.is_err() {return result(input::Disposition::Deferred,"write_journal");}
        let receipt=crate::plugin::Receipt {publication_id:publication.into(),generation:self.target.generation,session_id:self.target.binding.conversation.clone()};
        match store.answer(&expected.binding,&question.id,&expected.interaction_id,body,Some(receipt)).await {
            Ok(PluginReply::Accepted)=>result(input::Disposition::Accepted,"plugin_answered"),
            Ok(PluginReply::NotWritten|PluginReply::Unavailable)=>result(input::Disposition::Deferred,"plugin_not_written"),
            Ok(_)=>result(input::Disposition::Unknown,"plugin_control_uncertain"),
            Err(error)=>result(input::Disposition::Rejected,error.0),
        }
    }

    async fn permission(&mut self,id:&str,kind:&str,payload:Value)->Result<RuntimeReply,RuntimeError> {
        use crate::permission_control::{self,PermissionAction};
        let memory=self.options.permissions.clone().ok_or_else(||error("permission_store_unbound"))?;
        let original=json!({"operation_id":id,"kind":kind,"payload":payload});
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.generation!=self.target.generation {return Err(error("runtime_generation"));}
        if let Some(old)=state.operations.get(id) {
            if old.payload!=original {return Err(error("operation_payload"));}
            return serde_json::from_value(old.result.clone()).or_else(|_|Ok(reply(id,Disposition::Unknown,json!({"error_code":"recovered_operation","uncertain":true}))));
        }
        if self.loaned() || self.held() || self.cleared(&state) {return Err(error("permission_control_busy"));}
        let services=self.services(id,id,"");
        let driver=TerminalDriver::new(self.target.binding.clone(),services,self.options.io.clone(),self.options.limits.clone());
        let tracking=self.target.binding.conversation.clone();
        let captured_revision=memory.lock().map_err(|_|error("permission_store_poisoned"))?.revision(&tracking);
        let initial=permission_control::claude(&driver,PermissionAction::Read {probe:false}).await?;
        let current=initial["current"].as_str().ok_or_else(||error("erro_permissao_leitura"))?;
        memory.lock().map_err(|_|error("permission_store_poisoned"))?.observe_capture(&tracking,current,captured_revision).map_err(error)?;
        let cached=self.modes_cache.clone().filter(|_|kind=="permission_modes");
        let probe=kind=="permission_modes" && payload["probe"].as_bool().unwrap_or(false);
        let action=if kind=="set_permission_mode" {PermissionAction::Set {mode:payload["mode"].as_str().ok_or_else(||error("erro_permissao_invalida"))?.into()}}
            else {PermissionAction::Read {probe:probe && cached.is_none()}};
        // Como o Python (`_bypass_no_ciclo`): sem a flag no processo o bypass não está no Shift+Tab; girar o
        // modo às cegas só mudaria o que a pessoa tinha. Vai direto à reabertura.
        // `dontAsk` não sai pelo Shift+Tab sem perder o modo: também reabre sem tecla.
        let outside=matches!(&action,PermissionAction::Set {mode} if mode=="bypassPermissions") && (current=="dontAsk" || !bypass_in_cycle(self.target.metadata.agent_pid)?);
        self.action(Action::Prepare {id:id.into(),payload:original,entry_id:None}).await?;
        self.action(Action::BeginDispatch {id:id.into(),wire_id:format!("permission:{}:{id}",self.target.generation),staged:false}).await?;
        if !memory.lock().map_err(|_|error("permission_store_poisoned"))?.begin(&tracking) {
            // Fechada no diário: a repetição do mesmo ID responde a mesma causa, não `recovered_operation`.
            let refused=reply(id,Disposition::Rejected,json!({"ok":false,"current":current,"error_code":"permission_snapshot_absent"}));
            self.action(Action::Finish {id:id.into(),status:status(Disposition::Rejected),result:serde_json::to_value(&refused).unwrap()}).await?;
            return Ok(refused);
        }
        let controlled=if outside {
            let mut value=initial.clone();
            value["requires_reopen"]=json!(true);value["cycle_closed"]=json!(true);value["error_code"]=json!("permission_reopen_required");
            value["ok"]=json!(false);value["mode"]=json!(current);value["requested"]=json!("bypassPermissions");
            Ok(value)
        } else {permission_control::claude(&driver,action).await};
        let (mut value,mut disposition)=match controlled {
            Ok(value)=> {
                let disposition=if value["uncertain"]==true || value["current"].is_null() {Disposition::Unknown}
                    else if value["requires_reopen"]==true {Disposition::Deferred}
                    // A sonda que não voltou responde a lista com `restaurado:false`, como o Python.
                    else if value["ok"]==true || kind=="permission_modes" {Disposition::Accepted} else {Disposition::Rejected};
                (value,disposition)
            },
            Err(failure)=>(json!({"ok":false,"current":null,"error_code":failure.code,"uncertain":true}),Disposition::Unknown),
        };
        if let Some(modes)=cached {value["modes"]=modes;}
        else if probe && value["uncertain"]==false && value["modes"].as_array().is_some_and(|modes|!modes.is_empty()) {self.modes_cache=Some(value["modes"].clone());}
        // Como o Python: tecla sem efeito visível relê o rodapé e responde o modo que ficou, sem outra tecla.
        if disposition==Disposition::Unknown && let Ok(read)=permission_control::claude(&driver,PermissionAction::Read {probe:false}).await
            && let Some(mode)=read["current"].as_str() {
            value["current"]=json!(mode);value["last_confirmed"]=json!(mode);value["uncertain"]=json!(false);
            let requested=value["requested"].as_str().map(str::to_owned);
            if let Some(requested)=&requested {value["mode"]=json!(mode);value["ok"]=json!(requested==mode);}
            if value["restaurado"].is_boolean() {value["restaurado"]=json!(value["initial"]==mode);}
            disposition=if requested.as_deref().is_none_or(|requested|requested==mode) {Disposition::Accepted} else {Disposition::Rejected};
            if disposition==Disposition::Accepted {value["error_code"]=Value::Null;}
        }
        {
            let mut memory=memory.lock().map_err(|_|error("permission_store_poisoned"))?;
            if disposition==Disposition::Unknown {
                memory.finish_unknown(&tracking);
            } else if let Some(mode)=value["current"].as_str() {
                let view=memory.finish(&tracking,mode);
                value["previous_non_plan"]=json!(view.previous_non_plan);
            }
        }
        let result=reply(id,disposition,value);
        self.action(Action::Finish {id:id.into(),status:status(disposition),result:serde_json::to_value(&result).unwrap()}).await?;
        // Modo sem prova só segura o que depende dele (reabertura, classe dos recados); a entrega segue,
        // e o pedido recebe o código.
        self.mode_unproved=disposition==Disposition::Unknown;
        self.publish().await?;
        Ok(result)
    }

    /// O seletor do `/model` (`model_picker::drive`), com o payload já validado pela rota. Fica no diário
    /// como a troca de modo: o mesmo ID repetido responde o que ficou, sem digitar de novo.
    async fn model_effort(&mut self,id:&str,payload:Value)->Result<RuntimeReply,RuntimeError> {
        let original=json!({"operation_id":id,"kind":"set_model_effort","payload":payload});
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if state.generation!=self.target.generation {return Err(error("runtime_generation"));}
        if let Some(old)=state.operations.get(id) {
            if old.payload!=original {return Err(error("operation_payload"));}
            return serde_json::from_value(old.result.clone()).or_else(|_|Ok(reply(id,Disposition::Unknown,json!({"detail":"resultado incerto; confira o terminal da sessão","error_code":"recovered_operation"}))));
        }
        // O código que o app já conhece da recusa do empréstimo do Python.
        if self.loaned() || self.held() || self.cleared(&state) {
            return Err(RuntimeError::new("erro_opcao_nao_convergiu","o terminal da sessão está ocupado com outra operação; tente de novo"));
        }
        let driver=TerminalDriver::new(self.target.binding.clone(),self.services(id,id,""),self.options.io.clone(),self.options.limits.clone());
        self.action(Action::Prepare {id:id.into(),payload:original,entry_id:None}).await?;
        self.action(Action::BeginDispatch {id:id.into(),wire_id:format!("model:{}:{id}",self.target.generation),staged:false}).await?;
        let result=match crate::model_picker::drive(&driver,payload["model"].as_str(),payload["effort"].as_str(),payload["scope"]=="default").await {
            Ok(value)=>reply(id,Disposition::Accepted,value),
            Err(refusal)=>reply(id,if refusal.uncertain {Disposition::Unknown} else {Disposition::Rejected},json!({"detail":refusal.detail,"error_code":refusal.code})),
        };
        self.action(Action::Finish {id:id.into(),status:status(result.disposition),result:serde_json::to_value(&result).unwrap()}).await?;
        Ok(result)
    }

    /// `steer`: o "Orientar" de uma entrada, que passa pelo turno em curso em vez de esperar o Claude parar.
    async fn execute(&mut self,id:&str,kind:&str,mut payload:Value,entry:Option<String>,steer:bool)->Result<RuntimeReply,RuntimeError> {
        if !valid_operation_id(id) {return Err(error("operation_id"));}
        // Texto colado no Windows chega com `\r\n`; o `\r` é controle e o Claude Code só entende `\n`.
        let unix=payload["text"].as_str().filter(|t|kind=="input" && t.contains('\r')).map(|t|t.replace("\r\n","\n").replace('\r',"\n"));
        if let Some(text)=unix {payload["text"]=json!(text);}
        if kind=="lifecycle_guard" {
            if self.loaned() || self.held() || self.last_error.is_some() || !self.uncertain.is_empty() || self.unprovable || self.mode_unproved {return Err(error("lifecycle_busy"));}
            let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
            if self.cleared(&state) || state.terminal_write_blocked(&self.target.binding.conversation) {return Err(error("lifecycle_uncertain"));}
            let services=self.services(id,id,"");
            services.permission_guard(&self.target.binding).await.map_err(|e|error(e.0))?;
            let facts=services.facts(&self.target.binding).await.map_err(|e|error(e.0))?;
            if facts.binding!=self.target.binding {return Err(error("lifecycle_busy"));}
            // Os motivos do `_motivo_ocupada` do Python, com a frase que o app já conhece.
            if facts.open_question {return Err(error("erro_sessao_esperando_resposta"));}
            if !facts.idle {return Err(error("erro_sessao_trabalhando"));}
            // Na troca por limite a fila foi segurada para ir com a conversa à conta nova.
            if payload["queued_ok"]!=true && state.rows.iter().any(|row|row["delivered"]==false) {return Err(error("erro_fila_pendente"));}
            return Ok(reply(id,Disposition::Accepted,json!({"idle":true,"conversation":self.target.binding.conversation})));
        }
        if kind=="heal_guard" {
            self.heal_ready().await?;
            return Ok(reply(id,Disposition::Accepted,json!({"idle":true,"conversation":self.target.binding.conversation})));
        }
        if matches!(kind,"permission_modes"|"set_permission_mode") {return self.permission(id,kind,payload).await;}
        if kind=="set_model_effort" {return self.model_effort(id,payload).await;}
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
        let expected=if matches!(kind,"select"|"submit_selected"|"answer_questions"|"answer_chat") {Some(self.expected(&payload)?)}else{None};
        // Pergunta `ask:` do plugin que ele não pode receber agora (sem recibo, vida nova antes do `/pull`):
        // a resposta cai nas teclas do pane, como no Python. Permissão nunca: lá a recusa é de propósito.
        let mut keyboard_ask=false;
        if let Some(expected)=&expected {
            let question=matches!(kind,"answer_questions"|"answer_chat");
            if (question && expected.target.kind!=crate::interactions::InteractionKind::Question)
                || (!question && expected.target.kind==crate::interactions::InteractionKind::Question) {
                return Err(runtime_error(InteractionError::Changed));
            }
            if let Some(request_id)=expected.target.request_key.strip_prefix("plugin:").filter(|_|expected.semantic["source"]!="terminal") {
                if kind=="submit_selected" {return Err(runtime_error(InteractionError::Unavailable));}
                if payload.get("request_id").filter(|value|!value.is_null()).is_some_and(|value|value.as_str()!=Some(request_id)) {return Err(runtime_error(InteractionError::Changed));}
                match self.local.plugin_question(&self.target,expected).await {
                    Err(error) if error.0==InteractionError::Unavailable.code() && request_id.starts_with("ask:") && kind=="answer_questions" => keyboard_ask=true,
                    Err(error) => return Err(RuntimeError::new(error.0,"pergunta do plugin não conferida")),
                    Ok(_) => {},
                }
            } else {
                if expected.semantic["source"]!="terminal" {return Err(runtime_error(InteractionError::Unavailable));}
                if let Some(request_id)=payload.get("request_id").filter(|value|!value.is_null()) {
                    let target=crate::interactions::native_target(expected.target.kind.clone(),&expected.binding.conversation,request_id);
                    if target!=expected.target {return Err(runtime_error(InteractionError::Changed));}
                }
            }
        }
        // A trava segura só o texto, que cairia na conversa nova; Parar, teclas e respostas seguem com ela.
        if kind=="input" && self.cleared(&state) && !self.expire_clear().await {return Err(error("runtime_clear_barrier"));}
        let text=payload["text"].as_str().unwrap_or("");
        let prompt=kind=="input"; let slash=prompt && text.trim_start().starts_with('/');
        let row_id=if prompt && !slash {Some(entry.unwrap_or_else(||id.into()))}else{None};
        let prepared=self.action(Action::Prepare {id:id.into(),payload:original,entry_id:row_id.clone()}).await?;
        if let Some(disposition)=match prepared["status"].as_str() {
            Some("accepted"|"confirmed")=>Some(Disposition::Accepted),Some("rejected")=>Some(Disposition::Rejected),_=>None,
        } {
            return Ok(serde_json::from_value(prepared["result"].clone()).unwrap_or_else(|_|reply(id,disposition,Value::Null)));
        }
        let mut fresh=false;
        if let Some(row)=&row_id {
            if !state.rows.iter().any(|r|r["id"]==*row) {
                // Segurada: em idle a linha só aparece já entregue, sem piscar "Na fila"; o adiamento a publica no `Finish`.
                self.services("maintenance","maintenance","").action_held("queue",Action::Append {text:text.into(),delivered:false,ts:None,pre_transcript:payload["pre_transcript"]==true,entry_id:Some(row.clone())},true).await?;
                self.batch_blocked=false;
                fresh=true;
            }
        }
        if prompt && !is_clear(text) && state.terminal_write_blocked(&self.target.binding.conversation) {
            return self.defer(id,json!({"code":"terminal_write_barrier","queued":row_id.is_some(),"cleanup":"not_needed"})).await;
        }
        // Comando de barra com mensagens esperando no Hangar: não passa à frente delas nem as perde num `/clear`
        // (a troca de conversa esvazia a fila). Recusado na tela, como todo comando de barra adiado.
        if slash && !super::queue::waiting_ids(&state,self.target.created).is_empty() {
            return self.defer(id,json!({"code":"queue_waiting","queued":false,"cleanup":"not_needed"})).await;
        }
        if self.loaned() {
            // O Python está digitando no pane: a entrada com linha na fila espera e sai depois da
            // devolução; comando e controle, sem fila para esperar, são recusados com o código.
            let (disposition,status)=if row_id.is_some() {(Disposition::Deferred,Status::Deferred)} else {(Disposition::Rejected,Status::Rejected)};
            let result=reply(id,disposition,json!({"code":"keyboard_loan","queued":row_id.is_some(),"cleanup":"not_needed"}));
            self.action(Action::Finish {id:id.into(),status,result:serde_json::to_value(&result).unwrap()}).await?;
            return Ok(result);
        }
        // Mensagem nova não passa à frente de outra que ficou adiada: espera na fila e a drenagem entrega na ordem.
        if fresh && state.pending_ahead(self.target.created) {
            return self.defer(id,json!({"code":"queue_ahead","queued":true,"cleanup":"not_needed"})).await;
        }
        let root=row_id.as_deref().unwrap_or(id);
        // Antes da escrita: o transcript novo do `/clear` nasce logo depois do Enter.
        let dispatched_at=sample().epoch_s;
        let services=self.services(root,id,text);
        let mut driver=TerminalDriver::new(self.target.binding.clone(),services.clone(),self.options.io.clone(),self.options.limits.clone()).with_interaction_source(self.target.transcript.clone());
        if let Some(store)=&self.options.interactions {driver=driver.with_interactions(store.clone());}
        if matches!(kind,"key"|"navigation_key"|"interactive_key"|"terminal_input"|"interrupt") {driver=driver.controls();}
        if self.footer_misses.as_ref().is_some_and(|(row,misses)|row==root && *misses>=FOCUS_RETURN_TRIES) {driver=driver.footer_kept();}
        let facts=if prompt {Some(services.facts(&self.target.binding).await)}else{None};
        let current=facts.as_ref().is_none_or(|result|result.as_ref().is_ok_and(|facts|facts.binding==self.target.binding));
        if let Some(facts)=&facts {self.deliverable=facts.as_ref().is_ok_and(|facts|current && facts.ready && !facts.open_question);}
        // Com o Claude trabalhando a mensagem fica na fila do Hangar, onde o painel a apaga, edita e reordena.
        // O recado com socket nativo segue: é o canal do próprio Claude Code para o meio do turno.
        let ok=facts.as_ref().and_then(|result|result.as_ref().ok());
        let busy=ok.is_some_and(|f|self.turn_busy(f.idle));
        let started_turn=ok.is_some_and(|f|f.idle);
        let held=prompt && !slash && !steer && busy && ok.is_some_and(|f|!input::native_message(f,text));
        let mut result=if !current {reply(id,Disposition::Deferred,json!({"code":"terminal_facts","cleanup":"not_needed"}))}
            else if held {reply(id,Disposition::Deferred,json!({"code":"terminal_turn_hold","queued":true,"cleanup":"not_needed"}))}
            else if prompt && !slash && !ok.is_some_and(|f|f.ready && !f.open_question || f.native.is_some()) {reply(id,Disposition::Deferred,json!({"queued":true,"cleanup":"not_needed"}))}
            else {
                let bound=match row_id.is_some() {
                    true=>match self.receipt.capture(&self.target.transcript) {
                        Ok(cursor)=>self.action(Action::BindDispatch {id:id.into(),cursor:serde_json::to_value(cursor).unwrap()}).await.map(|_|()),
                        Err(_)=>Err(error("receipt_cursor")),
                    },
                    false=>Ok(()),
                };
                // A entrada só escreve depois do `writing` do escritor; antes disso, cair não a deixa incerta.
                let begun=match bound {
                    Ok(())=>self.action(Action::BeginDispatch {id:id.into(),wire_id:format!("terminal:{}:{id}",self.target.generation),staged:kind=="input"}).await.map(|_|()),
                    Err(failure)=>Err(failure),
                };
                // Falha com a projeção segurada: a linha que ficou na fila precisa aparecer.
                if let Err(failure)=begun {
                    if fresh && self.action(Action::EnsureProjection).await.is_err() {
                        tracing::warn!(key=%self.target.key,session=%self.target.name,code="queue_projection_failed","a linha segurada não voltou à projeção depois da falha da entrega");
                    }
                    return Err(failure);
                }
                let publication=format!("{}:{}:{id}",self.target.key,self.target.generation);
                let interrupted=if kind=="interrupt" {match &self.options.plugin {
                    Some(store)=>store.pending_all(&crate::terminal_facts::LocalTerminalFacts::binding(&self.target),std::time::Instant::now()).map_err(|failure|error(failure.0))?,None=>vec![],
                }} else {vec![]};
                let mut delivered=if let Some(expected)=expected.as_ref().filter(|expected|expected.target.request_key.starts_with("plugin:") && expected.semantic["source"]!="terminal") {
                    if keyboard_ask {match serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()) {
                        Ok(answers)=>driver.plugin_ask_keys(expected,&answers).await,
                        // Corpo torto recusa com o código, nunca derruba o executor.
                        Err(_)=>DeliveryResult {disposition:input::Disposition::Rejected,stage:input::DeliveryStage::Validate,cleanup:input::Cleanup::NotNeeded,
                            native:false,message_id:None,code:"invalid_answer".into(),draft:None,consumed:false},
                    }}
                    else {self.plugin_control(kind,&payload,expected,&publication,&services).await}
                } else {match kind {
                        // Foco fora do prompt: adia sem escrever, como o `overlay`; a linha volta à fila e
                        // o próximo tique tenta de novo.
                        // Só quando a entrega vai apertar tecla: o modo `User` e a nativa não passam pelo teclado,
                        // e a guarda só custaria leituras e atraso.
                        "input"=>match if facts.as_ref().is_some_and(|f|f.as_ref().is_ok_and(|f|!input::presses_keys(f,text))) {None} else {self.focus_guard(root,facts.as_ref().is_some_and(|f|f.as_ref().is_ok_and(|f|f.plugin_live))).await} {
                            Some(code)=>DeliveryResult {disposition:input::Disposition::Deferred,stage:input::DeliveryStage::Composer,cleanup:input::Cleanup::NotNeeded,
                                native:false,message_id:None,code:code.into(),draft:None,consumed:false},
                            None=>driver.prompt(text,&publication).await,
                        },
                        "steer"|"steer_queue"=>driver.steer().await,
                        "key"|"navigation_key"=>driver.key(payload["key"].as_str().unwrap(),false).await,
                        "interactive_key"=>driver.key(payload["key"].as_str().unwrap(),true).await,
                        "terminal_input"=>driver.text(text).await,
                        "select"=>match payload["text"].as_str() {
                            Some(text)=>driver.guarded_plan_feedback(expected.as_ref().unwrap(),payload["option"].as_u64().unwrap() as usize,text).await,
                            None=>driver.guarded_select(expected.as_ref().unwrap(),payload["option"].as_u64().unwrap() as usize,true).await,
                        },
                        "answer_questions"=>match serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()) {
                            Ok(answers)=>{
                                let expected=expected.as_ref().unwrap();
                                let keys=driver.guarded_answer(expected,&answers).await;
                                // As teclas falharam com o mesmo picker na tela: a reserva do Python, as respostas como mensagem.
                                let drive_failed=keys.disposition!=input::Disposition::Accepted && keys.stage!=input::DeliveryStage::Validate
                                    && InteractionError::from_code(&keys.code).is_none();
                                match if drive_failed {driver.guarded_answer_fallback(expected,&answers,&publication).await} else {None} {
                                    Some(fallback)=>{
                                        tracing::warn!(key=%self.target.key,session=%self.target.name,code=%keys.code,fallback=%fallback.code,"as teclas da resposta falharam; a resposta foi por texto");
                                        fallback
                                    }
                                    None=>keys,
                                }
                            }
                            Err(_)=>DeliveryResult {disposition:input::Disposition::Rejected,stage:input::DeliveryStage::Validate,cleanup:input::Cleanup::NotNeeded,
                                native:false,message_id:None,code:"invalid_answer".into(),draft:None,consumed:false},
                        },
                        "answer_chat"=>match serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()) {
                            Ok(answers)=>driver.guarded_answer_chat(expected.as_ref().unwrap(),&answers,&publication).await,
                            // Corpo torto recusa com o código, nunca derruba o executor.
                            Err(_)=>DeliveryResult {disposition:input::Disposition::Rejected,stage:input::DeliveryStage::Validate,cleanup:input::Cleanup::NotNeeded,
                                native:false,message_id:None,code:"invalid_answer".into(),draft:None,consumed:false},
                        },
                        "submit_selected"=>driver.guarded_submit_selected(expected.as_ref().unwrap()).await,
                        "interrupt"=>driver.interrupt(payload["clear"].as_bool().unwrap_or(false)).await,
                        _=>unreachable!(),
                    }};
                if delivered.disposition==input::Disposition::Accepted && !interrupted.is_empty() && let Some(store)=&self.options.plugin {
                    let binding=crate::terminal_facts::LocalTerminalFacts::binding(&self.target);
                    let notice=(|| {
                        let current=store.pending_all(&binding,std::time::Instant::now())?;
                        for question in &interrupted {
                            if current.iter().any(|pending|pending.id==question.id && pending.interaction_id==question.interaction_id) {store.interrupt(&binding,&question.id)?;}
                        }
                        Ok::<_,ServiceError>(())
                    })();
                    if let Err(failure)=notice {delivered.disposition=input::Disposition::Unknown;delivered.code=failure.0.into();}
                }
                delivery(id,delivered)
            };
        // O Esc saiu e o picker não fechou a tempo: o texto do "conversar" vai para a fila, que só
        // digita com o composer à vista, em vez de se perder.
        if kind=="answer_chat" && result.payload["code"]=="answer_chat_close_unproved" && let Some(expected)=&expected {
            let questions:Vec<String>=expected.semantic["questions"].as_array().into_iter().flatten().map(|q|q["question"].as_str().unwrap_or("").to_string()).collect();
            let text=crate::session_write::answer::chat_text(payload["answers"].as_array().map(Vec::as_slice).unwrap_or_default(),&questions);
            if !text.is_empty() {
                self.services("maintenance","maintenance","").action("queue",Action::Append {text,delivered:false,ts:None,pre_transcript:false,entry_id:Some(format!("{id}:chat"))}).await?;
                result=reply(id,Disposition::Deferred,json!({"code":"answer_chat_queued","queued":true,"cleanup":"not_needed"}));
            }
        }
        if let Some(expected)=&expected {
            result.payload["interaction_id"]=json!(expected.interaction_id);
            result.payload["interaction_binding"]=json!({"key":expected.binding.key,"conversation":expected.binding.conversation,"generation":expected.binding.generation});
            if result.disposition==Disposition::Accepted && matches!(kind,"answer_questions"|"answer_chat"|"submit_selected") {
                if let Some(revision)=&expected.source_revision {
                    result.payload["consumed_source_revision"]=json!(revision);
                    if crate::state::ask::consume_revision(&self.target.transcript,revision).is_err() {
                        result.disposition=Disposition::Unknown;
                        result.payload["code"]=json!("askq_consumption_unconfirmed");
                    }
                }
            }
        }
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
            let mut state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?.runtime_state.clone();
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
            // Resposta a menu incerta não trava a fila: a entrega seguinte só digita com o composer à vista,
            // e o menu que seguiu aberto a adia sozinho.
            if matches!(kind,"select"|"submit_selected"|"answer_questions") {self.publish().await?;}
            else {
                if row_id.is_some() {self.uncertain.push(id.into());} else if !clear_raised {self.unprovable=true;}
                self.enter_error(error("terminal_delivery_unknown")).await?;
            }
        }else{self.last_error=None; self.uncertain.clear(); self.unprovable=false; self.publish().await?;}
        if row_id.is_some() && result.disposition==Disposition::Accepted {
            self.confirm_until=Some(tokio::time::Instant::now()+CONFIRM_WINDOW); self.confirm_seen=None;
            // Entregue com o Claude parado, ela abre um turno que a próxima espera; a orientada no meio do turno, não.
            if started_turn && !self.options.turn_start_wait.is_zero() {self.turn_wait=Some(tokio::time::Instant::now()+self.options.turn_start_wait);}
        }
        Ok(result)
    }
    /// Sem o registro nativo do agente e sem o `/pull` do plugin, nada prova a conversa: a entrega não
    /// teria como provar nem limpar o que digitasse. Aparece depois do `stall_notice`, para não piscar na
    /// troca do `/pull`, e pede a reabertura quando a sessão para com mensagem esperando.
    async fn watch_proof(&mut self)->Result<(),RuntimeError> {
        let unproved=self.conversation_unproved().await;
        let now=tokio::time::Instant::now();
        let surfaced=unproved && now.duration_since(*self.unproved_since.get_or_insert(now))>=self.options.stall_notice;
        if !unproved {self.unproved_since=None;}
        if surfaced!=self.no_safe_delivery {
            self.no_safe_delivery=surfaced;
            if surfaced {
                tracing::warn!(key=%self.target.key,session=%self.target.name,code="no_safe_delivery",agent=self.target.metadata.agent_pid,
                    "registro nativo do agente ausente e plugin sem /pull: nada prova a conversa; a entrada não digita");
                self.diag.report("rust.no_safe_delivery",&self.target.name,"lifecycle_native_read","registro nativo do agente ausente e plugin sem /pull");
            } else {
                self.heal_requested=None;
                tracing::info!(key=%self.target.key,session=%self.target.name,code="no_safe_delivery_cleared","a prova da conversa voltou");
            }
            self.publish().await?;
        }
        if self.no_safe_delivery && self.heal_requested.is_none_or(|at|now.duration_since(at)>=super::gateway::HEAL_BACKOFF) {self.request_heal().await;}
        Ok(())
    }
    async fn conversation_unproved(&self)->bool {
        let binding=crate::terminal_facts::LocalTerminalFacts::binding(&self.target);
        if self.options.plugin.as_ref().is_some_and(|store|store.conversation_proved(&binding).unwrap_or(false)) {return false;}
        let (config,pid)=(self.target.metadata.config_dir.clone(),i64::from(self.target.metadata.agent_pid));
        // Qualquer falha de leitura (ausente, corrompido, de outro pid) deixa a conversa sem prova.
        tokio::task::spawn_blocking(move||super::lifecycle::read_native_conversation(&config,pid).is_err()).await.unwrap_or(true)
    }
    /// Reabrir aqui só com a sessão parada, sem pergunta, sem entrega incerta e com mensagem esperando.
    async fn heal_ready(&mut self)->Result<(),RuntimeError> {
        if self.loaned() || self.held() || !self.uncertain.is_empty() || self.unprovable || self.mode_unproved
            || self.last_error.as_deref().is_some_and(|code|code!="terminal_facts") {return Err(error("lifecycle_busy"));}
        if !self.no_safe_delivery {return Err(error("heal_not_needed"));}
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if self.cleared(&state) || state.terminal_write_blocked(&self.target.binding.conversation) {return Err(error("lifecycle_uncertain"));}
        if !state.rows.iter().any(|row|row["delivered"]==false) {return Err(error("heal_not_needed"));}
        if self.options.terms.as_ref().is_some_and(|terms|terms.is_active(&self.target.name)) {return Err(error("terminal_panel_active"));}
        self.local.idle_agent(&self.target).await.map_err(|failure|error(failure.0))
    }
    /// Espera a sessão parar; a reabertura roda fora do ator, que ela fecha e troca pela vida nova.
    async fn request_heal(&mut self) {
        let Some(registry)=self.options.lifecycle.as_ref().and_then(std::sync::Weak::upgrade) else {return};
        if self.heal_ready().await.is_err() {return;}
        self.heal_requested=Some(tokio::time::Instant::now());
        let target=self.target.clone();
        tokio::spawn(async move {let _=registry.heal_claude(target.name,target.key,target.generation).await;});
    }
    /// Adiada antes de escrever: fecha a operação com o motivo e o devolve a quem pediu.
    async fn defer(&mut self,id:&str,payload:Value)->Result<RuntimeReply,RuntimeError> {
        let result=reply(id,Disposition::Deferred,payload);
        self.action(Action::Finish {id:id.into(),status:Status::Deferred,result:serde_json::to_value(&result).unwrap()}).await?;
        Ok(result)
    }
    /// O Claude conta como trabalhando: ocupado de fato, ou logo depois de uma entrega comum que ainda não o
    /// mostrou ocupado. Vê-lo ocupado encerra a espera.
    fn turn_busy(&mut self,idle:bool)->bool {
        if !idle {self.turn_wait=None; return true;}
        let waiting=self.turn_wait.is_some_and(|until|tokio::time::Instant::now()<until);
        if !waiting {self.turn_wait=None;}
        waiting
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
        // A devolução do foco tem hora marcada (`focus_guard`): a espera crescente não pode passar dela.
        let due=self.away.as_ref().filter(|away|away.row==row && away.tries<FOCUS_RETURN_TRIES).map(|away|away.next);
        let next=due.map_or(now+stall.wait,|due|(now+stall.wait).min(due));
        let stall=self.stall.insert(Stall {next,..stall});
        // Sem área de transferência a mesma sessão de logon não volta esperando: o motivo aparece na hora.
        if !stall.surfaced && (now.duration_since(stall.since)>=self.options.stall_notice || stall.code=="clipboard_unavailable") {
            stall.surfaced=true;
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%stall.code,
                waited_s=now.duration_since(stall.since).as_secs(),"entrada terminal parada: o terminal recusa a entrega sem escrever");
        }
        repeated
    }
    async fn drain_once(&mut self,mut entry:Option<String>,steer:bool)->Result<Value,RuntimeError> {
        if self.batch_blocked && entry.is_none() {return Ok(json!({"drained":0,"batch_too_large":true}));}
        if self.loaned() {return Ok(json!({"drained":0,"keyboard_loan":true}));}
        if self.options.lifecycle.as_ref().and_then(std::sync::Weak::upgrade).is_some_and(|registry|registry.limit_held(&self.target.name)) {
            return Ok(json!({"drained":0,"limit_held":true}));
        }
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
        let facts=match services.facts(&self.target.binding).await {
            Ok(facts)=>{self.conversation_proved=true;facts},
            // O Claude Code só registra e abre a conversa depois de subir: a entrada espera, sem erro na sessão.
            // O prazo conta da idade do processo do agente: sessão adotada com o Claude de pé há tempo não espera.
            Err(failure) if failure.0=="terminal_conversation_unavailable" && !self.conversation_proved
                && sample().epoch_s-self.target.metadata.agent_birth<self.options.start_grace.as_secs_f64()=>return Ok(json!({"drained":0,"starting":true})),
            Err(_)=>return Err(error("terminal_facts")),
        };
        // O composer legível e sem pergunta é o que libera a escrita; o turno é conferido logo abaixo.
        self.deliverable=facts.binding==self.target.binding && facts.ready && !facts.open_question;
        // Composer que não se lê (o desenho mudou): não digita às cegas, mas a série de espera aparece na vista.
        if !self.deliverable && facts.binding==self.target.binding && facts.screen.as_deref().is_some_and(input::composer_unreadable) {
            if let Some(row)=state.rows.iter().find(|r|r["delivered"]==false).and_then(|r|r["id"].as_str()).map(str::to_owned) {
                let surfaced=|stall:&Option<Stall>|stall.as_ref().is_some_and(|s|s.surfaced);
                let before=surfaced(&self.stall);
                self.track_stall(&row,&reply("drain",Disposition::Deferred,json!({"code":"composer_unreadable","stage":"ready"})));
                if !before && surfaced(&self.stall) {self.publish().await?;}
            }
            return Ok(json!({"drained":0}));
        }
        if !self.deliverable || facts.binding!=self.target.binding {
            // Caixa ilegível ou com pergunta é espera normal: o motivo antigo sai da tela.
            if self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
            return Ok(json!({"drained":0}));
        }
        // A fila comum espera o turno e sai em lote. O "Orientar" (`steer`) passa por
        // cima, e o recado com socket nativo também, porque esse canal o Claude Code atende no meio do turno.
        if !steer && self.turn_busy(facts.idle) {
            // Os fatos da manutenção não sabem do socket (ele depende do texto): o recado candidato relê com o dele.
            let waiting=super::queue::waiting_ids(&state,self.target.created);
            let candidate=state.rows.iter().filter_map(|r|Some((r["id"].as_str()?,r["text"].as_str()?)))
                .find(|(id,text)|waiting.iter().any(|w|w==id) && entry.as_deref().is_none_or(|e|e==*id) && input::message_prefix(text).is_some())
                .map(|(id,text)|(id.to_owned(),text.to_owned()));
            let native=match candidate {
                Some((id,text))=>self.services(&id,&id,&text).facts(&self.target.binding).await.ok()
                    .filter(|f|f.binding==self.target.binding && input::native_message(f,&text)).map(|_|id),
                None=>None,
            };
            let Some(native)=native else {
                if self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
                return Ok(json!({"drained":0,"held_for_turn":true}));
            };
            entry=Some(native);
        }
        // Pergunta aberta que o pane não mostra (o menu rolou para fora da tela): as teclas do recado
        // navegariam o picker. A fila espera a resposta no transcript.
        // Sidecar mais velho que este processo é de uma vida que morreu na pergunta: retomada a conversa, o
        // Claude a dá por recusada sem disparar o PostToolUse, e o sidecar fica.
        let (config,stem,born)=(self.target.metadata.config_dir.clone(),self.target.binding.conversation.clone(),self.target.metadata.agent_birth);
        if tokio::task::spawn_blocking(move||{
            let sidecar=config.join(crate::list::facts_files::ASKQ_DIR).join(format!("{stem}.json"));
            let written=std::fs::metadata(&sidecar).and_then(|m|m.modified()).ok().and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|t|t.as_secs_f64());
            written.is_some_and(|written|written+1.0>=born) && crate::list::facts_files::open_question(Some(&stem),&[config]).is_some()
        }).await.map_err(|_|error("terminal_facts"))? {
            self.deliverable=false;
            if self.stall.take().is_some_and(|s|s.surfaced) {self.publish().await?;}
            return Ok(json!({"drained":0,"question_open":true}));
        }
        // A espera crescente só segura a escrita: os fatos (e o `deliverable`) seguem frescos, e o
        // composer que esvaziou (o dono enviou o rascunho) libera a tentativa na hora.
        if self.stall.as_ref().is_some_and(|s|tokio::time::Instant::now()<s.next) {
            let driver=TerminalDriver::new(self.target.binding.clone(),services.clone(),self.options.io.clone(),self.options.limits.clone());
            if !driver.composer_free().await {return Ok(json!({"drained":0,"stalled":true}));}
            if let Some(stall)=self.stall.as_mut() {stall.wait=Duration::ZERO;}
        }
        let directed=entry.is_some();
        let claim=match entry {
            Some(entry)=>Action::Claim {min_ts:self.target.created,limit:Some(1),entry_id:Some(entry)},
            None=>Action::ClaimBatch {min_ts:self.target.created},
        };
        let validate=Some(super::local_policy::batch_validator("claude-terminal".into(),json!({"modo":"fill","text":""})));
        let rows=match self.queue.exec_validated(self.target.generation,&format!("terminal:batch:{}",self.sequence.fetch_add(1,Ordering::Relaxed)),sample(),claim,false,validate).await {
            Ok(rows)=>rows,
            Err(failure) if super::queue::refusal(&failure)==Some(super::queue::BATCH_SIZE_MESSAGE)=>{
                self.batch_blocked=true;self.publish().await?;
                return Ok(json!({"drained":0,"batch_too_large":true}));
            },
            Err(_)=>return Err(error("queue_io")),
        };
        let Some(row)=rows.as_array().and_then(|r|r.first())else{return Ok(json!({"drained":0}));};
        let row_id=row["id"].as_str().ok_or_else(||error("queue_row"))?.to_string();
        // A sessão voltou a receber mensagem: o aviso da troca por limite que não aconteceu já passou.
        if let Some(sender)=&self.options.status {terminal_status::publish_limit(sender,None);}
        let attempt=format!("queue:{}:{}",row_id,self.sequence.fetch_add(1,Ordering::Relaxed));
        let outcome=self.execute(&attempt,"input",json!({"text":row["text"],"pre_transcript":row["pre_transcript"]==true}),Some(row_id),steer).await?;
        if directed && self.batch_blocked {self.batch_blocked=false;self.publish().await?;}
        Ok(json!({"drained":1,"reply":outcome}))
    }
    /// "Orientar" de uma entrada do painel. Parado: ela vai para a frente e sai na entrega comum. Trabalhando: é
    /// digitada (o Claude Code a enfileira) e promovida com `ctrl+x ctrl+s`, o mesmo caminho do chip antigo,
    /// que leva junto o que já estava na fila interna do Claude Code (limite dele).
    async fn steer_entry(&mut self,id:&str,payload:&Value)->Result<RuntimeReply,RuntimeError> {
        if !valid_operation_id(id) {return Err(error("operation_id"));}
        let entry=payload["entry_id"].as_str().filter(|entry|!entry.is_empty() && payload.as_object().is_some_and(|p|p.keys().all(|key|matches!(key.as_str(),"entry_id"|"expected_turn"))))
            .ok_or_else(||error("terminal_payload"))?.to_owned();
        // A que já saiu (ou nunca esteve) responde antes de ler a tela: nenhuma tecla.
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if !super::queue::waiting_ids(&state,self.target.created).contains(&entry) {
            return Ok(reply(id,Disposition::Rejected,json!({"code":"queue_entry_gone","cleanup":"not_needed"})));
        }
        let services=self.services("maintenance","maintenance","");
        let facts=services.facts(&self.target.binding).await.map_err(|_|error("terminal_facts"))?;
        if facts.binding!=self.target.binding {return Ok(reply(id,Disposition::Deferred,json!({"code":"queue_steer_not_delivered","cleanup":"not_needed"})));}
        if facts.open_question {return Ok(reply(id,Disposition::Rejected,json!({"code":"queue_steer_question_open","cleanup":"not_needed"})));}
        if !self.expected_work(payload) {return Ok(reply(id,Disposition::Rejected,json!({"code":"queue_steer_work_changed","cleanup":"not_needed"})));}
        if self.action(Action::Promote {entry_id:entry.clone()}).await?!=json!("moved") {
            return Ok(reply(id,Disposition::Rejected,json!({"code":"queue_entry_gone","cleanup":"not_needed"})));
        }
        self.publish().await?;
        // O turno pode começar entre as leituras: a entrega sempre passa pelo turno, e quem decide se promove é
        // a marca da fila interna na tela, lida depois.
        let idle=facts.idle;
        // Como no pedido de drenagem: a falha aparece na vista, além de voltar a quem pediu.
        let drained=match self.drain_once(Some(entry),true).await {Ok(value)=>value,Err(failure)=>{self.enter_error(failure.clone()).await?; return Err(failure);}};
        let outcome=serde_json::from_value::<RuntimeReply>(drained["reply"].clone()).ok();
        match outcome.map(|r|r.disposition) {
            Some(Disposition::Accepted)=>{},
            Some(Disposition::Unknown)=>return Ok(reply(id,Disposition::Unknown,json!({"code":"queue_steer_uncertain"}))),
            // Parado e sem entregar agora (caixa ilegível, foco num mod): ficou na frente e sai no próximo tique.
            None|Some(Disposition::Deferred) if idle=>return Ok(reply(id,Disposition::Accepted,json!({"steered":false,"drained":0}))),
            _=>return Ok(reply(id,Disposition::Deferred,json!({"code":"queue_steer_not_delivered","cleanup":"not_needed"}))),
        }
        let driver=TerminalDriver::new(self.target.binding.clone(),services,self.options.io.clone(),self.options.limits.clone());
        if !self.expected_work(payload) {return Ok(reply(id,Disposition::Accepted,json!({"steered":false,"drained":1})));}
        // A marca da fila pode demorar um quadro a aparecer depois do Enter; sem ela de vez, o Claude estava parado
        // (ou o turno acabou no meio) e já levou a mensagem: entregue igual, sem promover. Parado, a entrega abriu
        // o turno com ela: uma leitura basta, sem as esperas.
        let deadline=tokio::time::Instant::now()+if idle {STEER_HINT_WAIT_IDLE} else {STEER_HINT_WAIT};
        let mut steered=driver.steer().await;
        // Só o prazo esgotado é "a dica não veio": o turno acabando no meio entregou a mensagem como envio comum.
        let mut turn_ended=false;
        while steered.disposition==input::Disposition::Deferred && tokio::time::Instant::now()<deadline {
            tokio::time::sleep(self.options.limits.settle).await;
            if !self.expected_work(payload) {turn_ended=true; break;}
            steered=driver.steer().await;
        }
        // O ctrl+x ctrl+s pode ter ido sem se comprovar: nem "orientou" nem "a dica não veio", e quem clicou confere.
        if steered.disposition==input::Disposition::Unknown {
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%steered.code,"a mensagem entrou na fila do Claude Code, mas o ctrl+x ctrl+s não se comprovou");
            return Ok(reply(id,Disposition::Unknown,json!({"code":"queue_steer_uncertain"})));
        }
        let accepted=steered.disposition==input::Disposition::Accepted;
        // Trabalhando e sem a dica no prazo: a mensagem espera a próxima pausa do Claude, e quem clicou precisa saber.
        if steered.disposition==input::Disposition::Deferred && !turn_ended && !idle {
            tracing::warn!(key=%self.target.key,session=%self.target.name,code=%steered.code,"Orientar: a dica do ctrl+x ctrl+s não apareceu no prazo");
            return Ok(reply(id,Disposition::Accepted,json!({"steered":false,"drained":1,"code":"queue_steer_hint_missing"})));
        }
        Ok(reply(id,Disposition::Accepted,json!({"steered":accepted,"drained":1})))
    }

    fn working_revision(&self)->Option<u64> {
        self.options.plugin.as_ref()?.working_revision(&crate::terminal_facts::LocalTerminalFacts::binding(&self.target),std::time::Instant::now()).ok().flatten()
    }
    fn expected_work(&self,payload:&Value)->bool {
        payload.get("expected_turn").is_none_or(|expected|self.working_revision().is_some_and(|revision|*expected==json!({"working_revision":revision})))
    }
    /// `/steer` sem corpo (chip de app antigo): com mensagem esperando no Hangar, é o Orientar da primeira, que não
    /// pode ficar para trás da fila interna; sem nenhuma, promove a fila do Claude Code como antes. A resposta da
    /// entrada leva `entry_id`.
    async fn steer_first(&mut self,id:&str)->Result<RuntimeReply,RuntimeError> {
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        let Some(first)=super::queue::waiting_ids(&state,self.target.created).into_iter().next() else {
            return self.execute(id,"steer",json!({}),None,false).await;
        };
        let mut answer=self.steer_entry(id,&json!({"entry_id":first})).await?;
        answer.payload["entry_id"]=json!(first);
        Ok(answer)
    }
    async fn confirm_rows(&mut self)->Result<Value,RuntimeError> {
        let state=self.queue.snapshot().await.map_err(|_|error("queue_io"))?;
        if !state.rows.iter().any(|r|r["delivered"]==true && r["confirmed"]!=true){return Ok(json!({"confirmed":0}));}
        self.receipt.scan(&self.target.transcript).map_err(|_|error("receipt_scan"))?;
        let mut count=0;
        // A desistida também: a mensagem que chegou depois deixa de constar como "não chegou".
        let given_up=|op:&super::queue::Operation|op.status==Status::Rejected && op.result["payload"]["code"]=="delivery_given_up";
        for operation in state.operations.values().filter(|op|op.entry_id.is_some() && (matches!(op.status,Status::Accepted|Status::Unknown|Status::Dispatching) || given_up(op))) {
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
            let used=self.queue.snapshot().await.map_err(|_|error("queue_io"))?.used_occurrences.clone();
            let (Some(entry_id),Some((occurrence,normalized_text)))=(row["id"].as_str().map(str::to_owned),self.receipt.match_legacy(&row,&used)) else {continue;};
            // Falha numa legada não desfaz as despachadas já confirmadas: fica para a próxima rodada.
            match self.action(Action::ConfirmLegacy {entry_id,occurrence,normalized_text}).await {
                Ok(accepted)=>if accepted==true {count+=1;},
                Err(failure)=>tracing::warn!(key=%self.target.key,code=%failure.code,"confirmação de entrada legada falhou; segue na próxima rodada"),
            }
        }
        if count>0{self.publish().await?;}Ok(json!({"confirmed":count}))
    }
    /// A entrega incerta que segue sem prova depois do prazo, com o Claude parado, é desistida sozinha:
    /// parado, o que ele tivesse guardado já teria virado conversa. Nada é reenviado.
    async fn expire_uncertain(&mut self) {
        // Falha aqui adia a desistência para o próximo tique; o executor segue de pé e o diário diz por quê.
        let skip=|key:&str,code:&str,reason:&str| if crate::warn_limit::allow(Some(key),code) {
            tracing::warn!(key=%key,code=%code,"{reason}");
        };
        let state=match self.queue.snapshot().await {Ok(state)=>state, Err(_)=>{skip(&self.target.key,"give_up_queue_io","prazo da entrega incerta: fila ilegível, tenta no próximo tique"); return}};
        let owner=state.runtime_state["terminal_write_barrier"]["operation_id"].as_str().map(str::to_owned);
        let Some(entry)=owner.as_deref().and_then(|id|state.operations.get(id)).and_then(|op|op.entry_id.clone()) else {self.uncertain_since=None; return};
        let now=tokio::time::Instant::now();
        let since=match &self.uncertain_since {Some((seen,at)) if *seen==entry=>*at, _=>{self.uncertain_since=Some((entry,now)); return}};
        if now.duration_since(since)<self.options.give_up_after {return;}
        let services=self.services("maintenance","maintenance","");
        match services.facts(&self.target.binding).await {
            Ok(facts) if facts.binding==self.target.binding && facts.idle=>{},
            Ok(_)=>return,
            Err(_)=>{skip(&self.target.key,"give_up_facts","prazo da entrega incerta: fatos indisponíveis, sem saber se o Claude parou"); return},
        }
        // Texto ainda no composer é a própria entrega parada: desistir deixaria o dono com ele preso no prompt.
        let driver=TerminalDriver::new(self.target.binding.clone(),services,self.options.io.clone(),self.options.limits.clone());
        if !driver.composer_free().await {
            skip(&self.target.key,"give_up_composer_busy","prazo da entrega incerta: o composer tem texto; a desistência fica com o Descartar");
            return;
        }
        // Última chance da prova antes de desistir.
        if self.confirm_rows().await.is_err() {skip(&self.target.key,"give_up_receipt","prazo da entrega incerta: transcript ilegível, tenta no próximo tique"); return;}
        let id=format!("give_up:{entry}:{}",self.sequence.fetch_add(1,Ordering::Relaxed));
        let given=self.queue.exec(self.target.generation,&id,sample(),Action::GiveUp {entry_id:entry.clone()}).await;
        self.uncertain_since=None;
        if !matches!(given,Ok(ref done) if *done==json!(true)) {
            skip(&self.target.key,"give_up_refused","prazo da entrega incerta: a desistência não agiu; o prazo recomeça");
            return;
        }
        tracing::warn!(key=%self.target.key,session=%self.target.name,code="delivery_given_up",entry=%entry,
            waited_s=now.duration_since(since).as_secs(),"entrega incerta sem prova no prazo: desistida, sem reenviar; a fila segue");
        self.release_uncertain().await;
        if let Err(failure)=self.publish().await {tracing::warn!(key=%self.target.key,code=%failure.code,"vista da desistência não publicou");}
    }
    /// Entrega aceita sem prova (eco diferente, prompt barrado por hook) passado o prazo, com o Claude
    /// parado: vira desistida, que o Descartar tira. Nada é reenviado; a prova que chegar depois ainda a
    /// confirma (`confirm_rows`). A incerta que segura a escrita é do `expire_uncertain`.
    async fn expire_unproved(&mut self) {
        let Ok(state)=self.queue.snapshot().await else {return};
        if state.terminal_write_blocked(&self.target.binding.conversation) || self.cleared(&state) {return;}
        let (now,grace)=(sample().epoch_s,self.options.give_up_after.as_secs_f64());
        let due=state.rows.iter().any(|r|r["delivered"]==true && r["confirmed"]!=true && r["desistiu"]!=true && r["papel"]!="assistant" && r["steered"]!=true
            && ["dispatched_ts","claimed_ts","ts"].iter().find_map(|k|r[*k].as_f64()).unwrap_or(0.0)+grace<=now);
        if !due {return;}
        // Parado, o que o Claude tivesse enfileirado já teria virado conversa.
        if !self.services("maintenance","maintenance","").facts(&self.target.binding).await.is_ok_and(|facts|facts.binding==self.target.binding && facts.idle) {return;}
        if self.confirm_rows().await.is_err() {return;}
        let id=format!("give_up_unproved:{}",self.sequence.fetch_add(1,Ordering::Relaxed));
        match self.queue.exec(self.target.generation,&id,sample(),Action::GiveUpUnproved {now,grace}).await {
            Ok(given) if given["given_up"].as_array().is_some_and(|ids|!ids.is_empty())=>{
                tracing::warn!(key=%self.target.key,session=%self.target.name,code="delivery_given_up",entries=%given["given_up"],
                    "entrega aceita sem prova no prazo: desistida, sem reenviar");
                if let Err(failure)=self.publish().await {tracing::warn!(key=%self.target.key,code=%failure.code,"vista da desistência não publicou");}
            }
            Ok(_)=>{},
            Err(_)=>if crate::warn_limit::allow(Some(&self.target.key),"give_up_unproved_queue_io") {
                tracing::warn!(key=%self.target.key,code="give_up_unproved_queue_io","desistência da entrega sem prova não gravou; tenta no próximo tique");
            },
        }
    }
    /// Depois de uma desistência: a memória da entrega incerta só sai quando o disco não segura mais a escrita.
    async fn release_uncertain(&mut self) {
        let Ok(state)=self.queue.snapshot().await else {
            tracing::warn!(key=%self.target.key,code="give_up_queue_io","desistência gravada, mas a fila não releu; o erro de entrega incerta fica até a próxima leitura");
            return;
        };
        if state.terminal_write_blocked(&self.target.binding.conversation) {return;}
        self.uncertain.clear(); self.unprovable=false;
        if self.last_error.as_deref()==Some("terminal_delivery_unknown") {self.last_error=None;}
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
        let mut runtime=match self.queue.snapshot().await {Ok(state)=>state.runtime_state.clone(),Err(_)=>{self.clear_watch=Some(now+self.options.clear_wait); return false;}};
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
        "select"=>&["option","require_cursor","request_id","interaction_id","text"],"answer_questions"|"answer_chat"=>&["answers","request_id","interaction_id"],"interrupt"=>&["clear"],"submit_selected"=>&["interaction_id"],"steer_queue"=>&["entry_id"],_=>return Err(error("terminal_control")),
    };
    if !payload.as_object().is_some_and(|p|p.keys().all(|k|fields.contains(&k.as_str()))){return Err(error("terminal_payload"));}
    let valid=match kind {
        "input"=>payload["text"].as_str().is_some_and(valid_text) && payload.get("pre_transcript").is_none_or(Value::is_boolean),
        "terminal_input"=>payload["text"].as_str().is_some_and(safe_characters),
        "key"|"navigation_key"|"interactive_key"=>payload["key"].is_string(),
        "select"=>payload["option"].as_u64().is_some_and(|v|v>0 && v<=100) && payload.get("require_cursor").is_none_or(Value::is_boolean) && payload.get("text").is_none_or(|t|t.as_str().is_some_and(|t|valid_text(t) && !t.contains('\n'))) && payload.get("request_id").is_none_or(Value::is_string),
        "answer_questions"|"answer_chat"=>serde_json::from_value::<Vec<QuestionAnswer>>(payload["answers"].clone()).is_ok_and(|answers|!answers.is_empty() && answers.iter().all(|a|match a.kind {
            AnswerKind::Option=>!a.indices.is_empty() && a.indices.iter().all(|i|*i<100) && (a.multi||a.indices.len()==1) && !a.labels.is_empty(),
            AnswerKind::Text=>a.type_index.is_some_and(|i|i<100) && a.value.as_ref().is_some_and(|v|valid_text(v)&&!v.contains('\n')),
            AnswerKind::Chat=>a.chat_index.is_some_and(|i|i<100),
        })) && payload.get("request_id").is_none_or(Value::is_string),
        "interrupt"=>payload.get("clear").is_none_or(Value::is_boolean),"steer_queue"=>payload.get("entry_id").is_none_or(Value::is_string),_=>true,
    };
    if !valid || payload.get("interaction_id").is_some_and(|id|!id.is_null() && !id.is_string()) {return Err(error("terminal_payload"));}Ok(())
}

#[cfg(test)]
mod tests {
    /// argv ilegível (processo que sumiu, `/proc` negado) é erro com código, não "sem bypass".
    #[test]
    fn unreadable_argv_is_a_coded_error_not_a_reopen() {
        assert_eq!(super::bypass_in_cycle(u32::MAX).unwrap_err().code, "lifecycle_process_read");
        assert!(super::argv_has_bypass(&["claude".into(), "--permission-mode".into(), "bypassPermissions".into()]));
        assert!(!super::argv_has_bypass(&["claude".into()]));
    }
}
