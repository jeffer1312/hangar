//! E/S da vida comprovada: transporte existente e captura compartilhada.
use super::*;
use crate::terminal_input::{CommandRequest, ProcessIo, TerminalIo, TerminalDriver, InputLimits};
use crate::terminal_control::{TerminalPool,CaptureRequest};

#[derive(Clone)]
pub struct ProcessOtherIo {
    pool: TerminalPool,
    request: CaptureRequest,
    target: OtherTarget,
    processes: Arc<dyn crate::list::procs::ProcessView>,
    io: Arc<ProcessIo>,
}
impl ProcessOtherIo {
    pub fn new(target: &OtherTarget, options: &OtherOptions) -> Self {
        let request=CaptureRequest {consumer:format!("other:{}:{}",target.key,target.generation),name:target.name.clone(),provider:target.provider.as_str().into(),
            binding:format!("{}:{}",target.key,target.generation),target:target.pane_address.clone(),started:clock().epoch_s,lines:200,colors:true,join:false};
        Self {pool:options.pool.clone(),request,target:target.clone(),processes:options.processes.clone(),io:Arc::new(ProcessIo::default())}
    }
    fn driver(&self,target:&OtherTarget,guard:Option<&OtherWriteGuard>)->TerminalDriver {
        let mut binding=target.binding.clone();binding.pane=target.pane_address.clone();
        TerminalDriver::new(binding,Arc::new(super::super::terminal::NoFacts),Arc::new(GuardedIo {source:self.clone(),target:target.clone(),guard:guard.cloned()}),InputLimits::default()).controls()
    }
    async fn raw(&self,target:&OtherTarget,args:&[&str])->Result<Vec<u8>,RuntimeError> {
        let (program,prefix)=target.binding.mux_argv.split_first().ok_or_else(||failed("other_terminal_mux_missing"))?;
        let output=self.io.command(CommandRequest {program:program.clone(),args:prefix.iter().cloned().chain(args.iter().map(|s|s.to_string())).collect(),stdin:vec![]}).await
            .map_err(|_|failed("other_terminal_mux_unavailable"))?;
        if !output.success {return Err(failed("other_terminal_mux_refused"));}
        Ok(output.stdout)
    }
    async fn guarded_key(&self,target:&OtherTarget,key:&str,guard:Option<&OtherWriteGuard>)->Result<(),RuntimeError> {
        let (program,prefix)=target.binding.mux_argv.split_first().ok_or_else(||failed("other_terminal_mux_missing"))?;
        let io=GuardedIo {source:self.clone(),target:target.clone(),guard:guard.cloned()};
        let request=CommandRequest {program:program.clone(),args:prefix.iter().cloned().chain(["send-keys".into(),"-t".into(),target.pane_address.clone(),key.into()]).collect(),stdin:vec![]};
        let output=io.command(request).await.map_err(|error|if error.may_have_written {failed("other_terminal_delivery_unconfirmed")}else{RuntimeError::new(error.code,"o terminal não comprovou a escrita desta interação")})?;
        if !output.success {return Err(failed("other_terminal_delivery_unconfirmed"));}
        Ok(())
    }
    async fn prove_inner(&self,target:&OtherTarget)->Result<bool,RuntimeError> {
        let read=self.raw(target,&["display-message","-p","-t",&target.pane_address,"#{session_name}\t#{pane_id}\t#{session_created}\t#{pane_pid}"]).await?;
        let text=std::str::from_utf8(&read).map_err(|_|failed("other_terminal_pane_unreadable"))?;
        let fields:Vec<_>=text.trim_end().split('\t').collect();
        if fields.len()!=4 {return Err(failed("other_terminal_pane_unreadable"));}
        if fields[0]!=target.name || !target.binding.windows && fields[1]!=target.binding.pane
            || fields[2].parse::<u64>().ok()!=Some(target.binding.created) {return Ok(false);}
        let shell=fields[3].parse::<i64>().map_err(|_|failed("other_terminal_pane_unreadable"))?;
        if shell!=target.pane_pid {return Ok(false);}
        let (processes,target)=(self.processes.clone(),target.clone());
        tokio::task::spawn_blocking(move || {
            let argv=match processes.argv_checked(target.agent_pid) {
                Ok(argv)=>argv,
                Err(error) if crate::list::procs::vanished(&error)=>return Ok(false),
                Err(_)=>return Err(failed("other_terminal_process_unreadable")),
            };
            if argv.is_empty() {return Ok(false);}
            let birth=processes.start_time(target.agent_pid).ok_or_else(||failed("other_terminal_process_unreadable"))?;
            if !crate::terminal_facts::same_birth(birth,target.agent_birth) || crate::list::discover::provider_from_argv(&argv)!=Some(target.provider.as_str()) {return Ok(false);}
            if !target.binding.conversation.is_empty() {
                let found=locate_source(&target,&*processes).ok_or_else(||failed("other_terminal_conversation_unavailable"))?;
                if found!=target.transcript {return Err(failed("other_terminal_conversation_changed"));}
            }
            if shell==target.agent_pid {return Ok(true);}
            let children=processes.children(Duration::ZERO).map_err(|_|failed("other_terminal_process_unreadable"))?;
            let mut stack=vec![shell];let mut seen=std::collections::HashSet::new();
            while let Some(pid)=stack.pop() {
                if !seen.insert(pid) {continue;}
                if pid==target.agent_pid {return Ok(true);}
                if let Some(children)=children.get(&pid) {stack.extend(children.iter().copied());}
            }
            Ok(false)
        }).await.map_err(|_|failed("other_terminal_process_worker"))?
    }
}
impl OtherIo for ProcessOtherIo {
    fn prove<'a>(&'a self,target:&'a OtherTarget)->BoxFuture<'a,Result<bool,RuntimeError>> {Box::pin(self.prove_inner(target))}
    fn capture<'a>(&'a self)->BoxFuture<'a,Result<OtherFrame,RuntimeError>> {Box::pin(async move {
        let styled=if self.target.binding.windows {
            let output=self.raw(&self.target,&["capture-pane","-p","-e","-t",&self.target.pane_address,"-S","-200"]).await?;
            let text=String::from_utf8(output).map_err(|_|failed("other_terminal_capture_unreadable"))?;
            if text.contains('\u{fffd}') {return Err(failed("other_terminal_capture_unreadable"));}
            text
        }else{self.pool.capture(self.request.clone()).await.map_err(|_|failed("other_terminal_capture_unavailable"))?.text};
        Ok(OtherFrame::from_styled(styled))
    })}
    fn write<'a>(&'a self,target:&'a OtherTarget,value:&'a str,literal:bool,guard:Option<&'a OtherWriteGuard>)->BoxFuture<'a,Result<(),RuntimeError>> {Box::pin(async move {
        if !self.prove_inner(target).await? {return Err(failed("other_terminal_life_changed"));}
        let driver=self.driver(target,guard);
        if literal && target.binding.windows && value.contains('\n') {
            // Uma recusa posterior não desfaz bytes já enviados ao composer.
            let mut effects=false;
            for (index,line) in value.split('\n').enumerate() {
                if index>0 {
                    self.guarded_key(target,"C-j",guard).await.map_err(|error|if effects {failed("other_terminal_delivery_unconfirmed")}else{error})?;
                    effects=true;
                }
                if !line.is_empty() {
                    let result=driver.text(line).await;
                    if result.disposition!=crate::terminal_input::Disposition::Accepted {
                        return Err(write_failure(&result,effects));
                    }
                    effects=true;
                }
            }
            return Ok(());
        }
        if !literal && matches!(value,"C-s"|"C-j") {
            self.guarded_key(target,value,guard).await?;
            return Ok(());
        }
        let result=if literal {driver.text(value).await}else{driver.key(value,true).await};
        if result.disposition==crate::terminal_input::Disposition::Accepted {Ok(())}else{Err(write_failure(&result,false))}
    })}
    fn release<'a>(&'a self)->BoxFuture<'a,()> {Box::pin(async move {let _=self.pool.release(&self.request.consumer).await;})}
}

fn write_failure(result:&crate::terminal_input::DeliveryResult,effects:bool)->RuntimeError {
    if effects || result.disposition==crate::terminal_input::Disposition::Unknown {failed("other_terminal_delivery_unconfirmed")}
    else {RuntimeError::new(&result.code,"o terminal não confirmou a escrita")}
}

/// A mesma fonte nativa da descoberta: bilhete fresco e reserva pelo ambiente do processo.
pub(super) fn locate_source(target:&OtherTarget,processes:&dyn crate::list::procs::ProcessView)->Option<std::path::PathBuf> {
    crate::list::discover_other::ticket_transcript(target.provider.as_str(),&target.binding.pane,Some(target.pane_pid),&target.cwd,processes,&target.dirs,&mut crate::list::discover::Problems::default()).map(std::path::PathBuf::from)
}

#[derive(Clone)]
struct GuardedIo {
    source: ProcessOtherIo,
    target: OtherTarget,
    guard: Option<OtherWriteGuard>,
}
impl GuardedIo {
    async fn check(&self)->Result<(),crate::terminal_input::IoFailure> {
        use crate::terminal_input::IoFailure;
        if !self.source.prove_inner(&self.target).await.map_err(|_|IoFailure {code:"other_terminal_life_changed",may_have_written:false})? {
            return Err(IoFailure {code:"other_terminal_life_changed",may_have_written:false});
        }
        let Some(expected)=self.guard.clone() else{return Ok(());};
        let target=self.target.clone();
        let current=tokio::task::spawn_blocking(move||read_pending_interaction(&target)).await
            .map_err(|_|IoFailure {code:"erro_interacao_indisponivel",may_have_written:false})?
            .map_err(|_|IoFailure {code:"erro_interacao_indisponivel",may_have_written:false})?;
        if current.as_ref().is_some_and(|current|current.target==expected.target && current.semantic==expected.semantic) {Ok(())}
        else {Err(IoFailure {code:"erro_interacao_desatualizada",may_have_written:false})}
    }
}
impl crate::terminal_input::TerminalIo for GuardedIo {
    fn command<'a>(&'a self,request:crate::terminal_input::CommandRequest)->crate::terminal_input::IoFuture<'a,crate::terminal_input::CommandOutput> {
        Box::pin(async move {
            let effect=request.args.iter().any(|arg|matches!(arg.as_str(),"send-keys"|"paste-buffer"));
            if !effect {return self.source.io.command(request).await;}
            let output=self.source.io.command_guarded(request,||Box::pin(self.check())).await?;
            // Depois da tecla que responde, a pergunta sumir é a entrega: aí só a vida é reconferida.
            let checked=if self.guard.as_ref().is_some_and(|guard|guard.closing) {GuardedIo {guard:None,..self.clone()}.check().await} else {self.check().await};
            if checked.is_err() {return Err(crate::terminal_input::IoFailure {code:"other_terminal_delivery_unconfirmed",may_have_written:true});}
            Ok(output)
        })
    }
    fn socket<'a>(&'a self,descriptor:&'a crate::terminal_input::NativeMessage,envelope:Vec<u8>)->crate::terminal_input::IoFuture<'a,crate::terminal_input::WriteOutcome> {
        Box::pin(async move {self.check().await?;self.source.io.socket(descriptor,envelope).await})
    }
}
