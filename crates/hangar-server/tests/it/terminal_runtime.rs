use hangar_server::runtime::{actor::PolicyClient,protocol::{RuntimeCommand,OperationKind,ClockSample},queue::{self,QueueActor,Store,Action},terminal::{TerminalActor,TerminalTarget,TerminalOptions}};
use hangar_server::terminal_input::*;
use hangar_server::mods::click::{PaneOp,PaneReply};
use hangar_server::runtime::terminal_status::{StatusSender, TerminalStatus};
use serde_json::{Value,json};
use std::sync::{Arc,Mutex,atomic::AtomicU64};
use std::time::Duration;
use tokio::sync::{broadcast,Notify};

struct Io { native_record:Mutex<Option<(std::path::PathBuf,u32,std::path::PathBuf)>>, idle:Arc<std::sync::atomic::AtomicBool>, ready:Arc<std::sync::atomic::AtomicBool>, native:Arc<std::sync::atomic::AtomicBool>, generation:Arc<AtomicU64>, binding_generation:AtomicU64, capture_error:std::sync::atomic::AtomicBool, calls:Mutex<Vec<CommandRequest>>, text:Mutex<String>, gate:Notify, blocked:std::sync::atomic::AtomicBool, fail_write:std::sync::atomic::AtomicBool, fail_enter:std::sync::atomic::AtomicBool, rotate_enter:std::sync::atomic::AtomicBool, conversation:Arc<Mutex<String>>, socket_calls:Mutex<Vec<Vec<u8>>>, ghost:Mutex<String>, hold_capture:std::sync::atomic::AtomicBool, hold_skip:std::sync::atomic::AtomicUsize, mods_screen:Mutex<Option<String>>, ring_keys:std::sync::atomic::AtomicUsize, ring_returns:std::sync::atomic::AtomicBool, swallow:std::sync::atomic::AtomicBool, footer:std::sync::atomic::AtomicBool, panes:Mutex<Vec<String>>, created:AtomicU64, switch_at:Mutex<Option<(usize,&'static str)>>, projection_at_facts:Mutex<Option<(std::path::PathBuf,Vec<String>)>>, send_now_hint:std::sync::atomic::AtomicBool }
impl Io { fn new()->Self { Self { projection_at_facts:Mutex::new(None),native_record:Default::default(),idle:Arc::new(std::sync::atomic::AtomicBool::new(true)),ready:Arc::new(std::sync::atomic::AtomicBool::new(true)),native:Default::default(),generation:Arc::new(AtomicU64::new(1)),binding_generation:AtomicU64::new(1),capture_error:Default::default(),calls:Mutex::new(vec![]),text:Mutex::new(String::new()),gate:Notify::new(),blocked:std::sync::atomic::AtomicBool::new(false),fail_write:std::sync::atomic::AtomicBool::new(false),fail_enter:std::sync::atomic::AtomicBool::new(false),rotate_enter:std::sync::atomic::AtomicBool::new(false),conversation:Arc::new(Mutex::new("sid".into())),socket_calls:Mutex::new(vec![]),ghost:Mutex::new(String::new()),hold_capture:std::sync::atomic::AtomicBool::new(false),hold_skip:Default::default(),mods_screen:Mutex::new(None),ring_keys:Default::default(),ring_returns:Default::default(),swallow:Default::default(),footer:Default::default(),panes:Mutex::new(vec![]),created:AtomicU64::new(1),switch_at:Mutex::new(None),send_now_hint:std::sync::atomic::AtomicBool::new(false) } } }
impl Io {
    fn refresh_record(&self) {
        if let Some((path,pid,socket))=&*self.native_record.lock().unwrap() {
            let sid=if self.generation.load(std::sync::atomic::Ordering::Acquire)!=self.binding_generation.load(std::sync::atomic::Ordering::Acquire) {"changed-sid".to_owned()} else {self.conversation.lock().unwrap().clone()};
            std::fs::write(path,serde_json::to_vec(&json!({"pid":pid,"sessionId":sid,"status":if self.idle.load(std::sync::atomic::Ordering::Acquire){"idle"}else{"busy"},
                "messagingSocketPath":if self.native.load(std::sync::atomic::Ordering::Acquire){Some(socket)}else{None}})).unwrap()).unwrap();
        }
    }
}
impl TerminalIo for Io {
    fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput> { Box::pin(async move {
        let cmd=r.args[0].clone();
        let (count,written)={let mut calls=self.calls.lock().unwrap();let written=calls.iter().any(|c|c.args.contains(&"-l".into()));calls.push(r.clone());(calls.len(),written)};
        // Os fatos são lidos pela tela: a vista que existia em cada captura antes da primeira escrita.
        if cmd=="capture-pane" && !written && let Some((path,seen))=&mut *self.projection_at_facts.lock().unwrap() {
            seen.push(std::fs::read_to_string(path).unwrap_or_default());
        }
        // `switch_at`: na chamada N, a vida (conversa) ou o pane trocam, e ficam trocados.
        match *self.switch_at.lock().unwrap() {
            Some((at,"life")) if at==count=>{self.generation.fetch_add(1,std::sync::atomic::Ordering::AcqRel);},
            Some((at,"pane")) if at==count=>{self.created.store(2,std::sync::atomic::Ordering::Release);},
            _=>{},
        }
        self.refresh_record();
        let created=self.created.load(std::sync::atomic::Ordering::Acquire);
        let stdout=match cmd.as_str() {
            "display-message" if r.args.last().is_some_and(|a|a=="#{pane_pid}\t#{session_name}\t#{pane_id}\t#{session_created}")=>format!("{}\tsession\t%1\t{created}\n",std::process::id()).into_bytes(),
            "display-message" if r.args.last().is_some_and(|a|a==MODS_FORMATS)=>b"1|0|0|80|12\n".to_vec(),
            "display-message"=>format!("session\t%1\t{created}\n").into_bytes(),
            // `panes`: telas em sequência, uma por captura; a última fica.
            "capture-pane" if !self.panes.lock().unwrap().is_empty()=>{let mut panes=self.panes.lock().unwrap();(if panes.len()>1 {panes.remove(0)} else {panes[0].clone()}).into_bytes()},
            "capture-pane" if self.capture_error.load(std::sync::atomic::Ordering::Acquire)=>return Err(IoFailure {code:"capture_failed",may_have_written:false}),
            "capture-pane" if !self.ready.load(std::sync::atomic::Ordering::Acquire)=>b"loading\n".to_vec(),
            // `hold_skip`: capturas que passam antes de segurar (a leitura dos fatos vem antes do despacho).
            "capture-pane" if self.hold_capture.load(std::sync::atomic::Ordering::Acquire) && self.hold_skip.fetch_update(std::sync::atomic::Ordering::SeqCst,std::sync::atomic::Ordering::SeqCst,|n|n.checked_sub(1)).is_err()=>std::future::pending().await,
            // A tela dos mods (só a parte visível, sem `-S`), quando o teste a define.
            "capture-pane" if !r.args.contains(&"-S".into()) && self.mods_screen.lock().unwrap().is_some()=>self.mods_screen.lock().unwrap().clone().unwrap().into_bytes(),
            "capture-pane"=>{let text=self.text.lock().unwrap().clone();let ghost=self.ghost.lock().unwrap().clone();
                // O fantasma é rascunho que o Ctrl+S não guarda: o composer fica ocupado.
                // `footer`: o foco no painel de agentes, abaixo do composer, que o Esc não devolve.
                let footer=if self.footer.load(std::sync::atomic::Ordering::Acquire) {"  ↑/↓ to select\n\n❯ ● main\n  ◯ general-purpose  sleep\n"} else {""};
                // `send_now_hint`: a dica da fila interna do Claude Code, que o Orientar procura antes do ctrl+x ctrl+s.
                let hint=if self.send_now_hint.load(std::sync::atomic::Ordering::Acquire) {"  ctrl+x ctrl+s to send now\n"} else {""};
                format!("────────────────────────────────\n❯ {}\n────────────────────────────────\n{hint}{footer}",if text.is_empty(){ghost}else{text}).into_bytes()},
            // O `ctrl+x tab` da devolução do foco: com `ring_returns`, o foco volta ao prompt.
            "send-keys" if r.args.iter().any(|a|a=="C-x")=>{
                self.ring_keys.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                if self.ring_returns.load(std::sync::atomic::Ordering::Acquire) {*self.mods_screen.lock().unwrap()=None;}
                vec![]
            },
            "paste-buffer"=>{
                let bytes=self.calls.lock().unwrap().iter().rev().find(|call|call.args[0]=="load-buffer").unwrap().stdin.clone();
                *self.text.lock().unwrap()=String::from_utf8(bytes).unwrap();
                vec![]
            },
            "send-keys"=>{
                if self.blocked.load(std::sync::atomic::Ordering::Acquire) { self.gate.notified().await; }
                self.refresh_record();
                let text=r.args.last().unwrap();
                if text=="\r" || text=="C-u" { self.text.lock().unwrap().clear(); if text=="\r" && self.rotate_enter.load(std::sync::atomic::Ordering::Acquire){*self.conversation.lock().unwrap()="new-sid".into();} if text=="\r" && self.fail_enter.load(std::sync::atomic::Ordering::Acquire){return Err(IoFailure {code:"enter_uncertain",may_have_written:true});} }
                // `swallow`: o texto vai para onde está o foco (o painel de agentes), não para o composer.
                else if r.args.contains(&"-l".into()) && self.swallow.load(std::sync::atomic::Ordering::Acquire) {}
                else if r.args.contains(&"-l".into()) {
                    *self.text.lock().unwrap()=text.clone();
                    if self.fail_write.load(std::sync::atomic::Ordering::Acquire){return Err(IoFailure {code:"partial_write",may_have_written:true});}
                }
                vec![]
            }, _=>vec![]
        };
        self.refresh_record();
        Ok(CommandOutput {success:true,stdout})
    }) }
    fn socket<'a>(&'a self,_:&'a NativeMessage,envelope:Vec<u8>)->IoFuture<'a,WriteOutcome> { Box::pin(async move {self.socket_calls.lock().unwrap().push(envelope);Ok(WriteOutcome::Unknown)}) }
}
const WAIT:Duration=Duration::from_secs(10);
/// Processo do teste que morre junto com ele, inclusive quando uma asserção falha antes do fim.
struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
fn plugin_instance()->String {
    let mut stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let mut instance=String::new();while stamp>0 {instance.insert(0,char::from_digit((stamp%36) as u32,36).unwrap());stamp/=36;}instance.push_str("-fixture");instance
}
struct Fixture { _dir:tempfile::TempDir,target:TerminalTarget,policy:PolicyClient,io:Arc<Io>, idle:Arc<std::sync::atomic::AtomicBool>, ready:Arc<std::sync::atomic::AtomicBool>, generation:Arc<AtomicU64>, native:Arc<std::sync::atomic::AtomicBool>, unknown:Arc<std::sync::atomic::AtomicBool>, focus_plugin:Arc<std::sync::atomic::AtomicBool>, fill_plugin:Arc<std::sync::atomic::AtomicBool>, repull_ms:Arc<AtomicU64>, plugin:Arc<hangar_server::plugin::Store>, instance:String, plugin_task:Mutex<Option<tokio::task::JoinHandle<()>>>, plugin_requests:Arc<Mutex<Vec<Value>>>,status:StatusSender, turn_start_ms:AtomicU64 }
impl Fixture {
    async fn new()->Self {
        let dir=tempfile::tempdir().unwrap();let io=Arc::new(Io::new());let config=dir.path().join("config");
        let transcript=config.join("projects/project/sid.jsonl");std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();std::fs::create_dir_all(config.join("sessions")).unwrap();
        let pid=std::process::id();let metadata=hangar_server::terminal_facts::TerminalMetadata {config_dir:config.clone(),agent_pid:pid,agent_birth:hangar_server::terminal_facts::agent_birth(pid).unwrap(),legacy_import_after:Some(1.0),fingerprint:"fixture-life".into(),origin:"uds:fixture-inbox".into()};
        let binding=TerminalBinding {name:"session".into(),pane:"%1".into(),conversation:"sid".into(),generation:1,created:1,mux_argv:vec!["fake".into()],windows:false,clipboard_lock_path:None};
        let target=TerminalTarget {key:"key".into(),generation:1,name:"session".into(),binding,lease_path:dir.path().join("lease"),state_path:dir.path().join("state"),projection_dir:dir.path().join("projection"),transcript,created:1.0,plugin_key:None,metadata};
        std::fs::write(&target.transcript,"").unwrap();let socket=config.join("fixture.sock");std::fs::write(&socket,"").unwrap();
        *io.native_record.lock().unwrap()=Some((config.join("sessions").join(format!("{pid}.json")),pid,socket));io.refresh_record();
        let (idle,ready,generation,native)=(io.idle.clone(),io.ready.clone(),io.generation.clone(),io.native.clone());
        let unknown=Arc::new(std::sync::atomic::AtomicBool::new(false));let focus_plugin=Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (fill_plugin,repull_ms)=(Arc::new(std::sync::atomic::AtomicBool::new(false)),Arc::new(AtomicU64::new(0)));
        let plugin=Arc::new(hangar_server::plugin::Store::new(Arc::new(hangar_server::interactions::InteractionStore::default())));
        Self {_dir:dir,target,policy:PolicyClient::new("127.0.0.1:9".parse().unwrap(),"test".into()),io,idle,ready,generation,native,unknown,focus_plugin,fill_plugin,repull_ms,plugin,instance:plugin_instance(),plugin_task:Default::default(),plugin_requests:Default::default(),status:tokio::sync::watch::channel(TerminalStatus::default()).0,turn_start_ms:AtomicU64::new(0)}
    }
    fn owner(&self,receipt:bool) {
        let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&self.target);
        let mut modes=vec!["fill".into()];if receipt {modes.push("receipt_v2".into());}
        drop(self.plugin.begin_pull(&binding,hangar_server::plugin::Pull {instance:self.instance.clone(),modos:modes,estado:Some("idle".into()),session_id:Some(binding.conversation.clone())},std::time::Instant::now()).unwrap());
    }
    fn start_plugin(&self) {
        use hangar_server::plugin::{Pull,Receipt};
        let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&self.target);
        if let Some(task)=self.plugin_task.lock().unwrap().take() {task.abort();}
        self.plugin.interactions().bind(&binding).unwrap();self.plugin.bind(&binding).unwrap();
        self.io.binding_generation.store(self.target.generation,std::sync::atomic::Ordering::Release);self.io.refresh_record();
        if !self.unknown.load(std::sync::atomic::Ordering::Acquire) && !self.focus_plugin.load(std::sync::atomic::Ordering::Acquire) && !self.fill_plugin.load(std::sync::atomic::Ordering::Acquire) {return;}
        let (plugin,unknown,focus,io,requests,path)=(self.plugin.clone(),self.unknown.clone(),self.focus_plugin.clone(),self.io.clone(),self.plugin_requests.clone(),self.target.state_path.clone());
        let (fill,repull)=(self.fill_plugin.clone(),self.repull_ms.clone());
        let instance=self.instance.clone();let task=tokio::spawn(async move {
            while unknown.load(std::sync::atomic::Ordering::Acquire) || focus.load(std::sync::atomic::Ordering::Acquire) || fill.load(std::sync::atomic::Ordering::Acquire) {
                let mut modes=vec!["fill".into(),"user".into(),"receipt_v2".into()];if focus.load(std::sync::atomic::Ordering::Acquire) {modes.push("focus".into());}
                let payload=plugin.begin_pull(&binding,Pull {instance:instance.clone(),modos:modes,estado:Some("idle".into()),session_id:Some(binding.conversation.clone())},std::time::Instant::now()).unwrap().wait().await;
                if !payload["publication_id"].is_string() {continue;}
                let state:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();let publication=payload["publication_id"].as_str().unwrap();let operation=publication.strip_prefix("key:1:").unwrap_or(publication);
                let mut recorded=payload.clone();recorded["_writing"]=state["operations"][operation]["wire_attempts"][format!("terminal:{}:{operation}",binding.generation)]["status"].clone();requests.lock().unwrap().push(recorded);
                let receipt=Receipt {publication_id:publication.into(),generation:payload["generation"].as_u64().unwrap(),session_id:payload["session_id"].as_str().unwrap().into()};
                let mode:PluginMode=serde_json::from_value(payload["modo"].clone()).unwrap();let ok=!unknown.load(std::sync::atomic::Ordering::Acquire) || mode==PluginMode::Focus;
                if mode==PluginMode::Focus {*io.mods_screen.lock().unwrap()=None;}else if ok && mode==PluginMode::Fill {*io.text.lock().unwrap()=payload["text"].as_str().unwrap().into();}
                plugin.acknowledge(&binding,Some(&receipt),if mode==PluginMode::Focus {PluginMode::Fill}else{mode},ok).unwrap();
                // O plugin real leva um tempo entre o aviso e o `/pull` seguinte.
                tokio::time::sleep(Duration::from_millis(repull.load(std::sync::atomic::Ordering::Acquire))).await;
            }
        });*self.plugin_task.lock().unwrap()=Some(task);
    }
    fn start(&self)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_with_events(broadcast::channel(128).0)
    }
    fn start_with_events(&self,events:broadcast::Sender<hangar_server::runtime::protocol::RuntimeEvent>)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_full(events,Duration::from_secs(30))
    }
    fn start_full(&self,events:broadcast::Sender<hangar_server::runtime::protocol::RuntimeEvent>,stall_notice:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_returning(events,stall_notice,TerminalOptions::default().focus_return)
    }
    fn start_returning(&self,events:broadcast::Sender<hangar_server::runtime::protocol::RuntimeEvent>,stall_notice:Duration,focus_return:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_options(events,stall_notice,focus_return,TerminalOptions::default().clear_wait)
    }
    fn start_clear(&self,clear_wait:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_options(broadcast::channel(128).0,Duration::from_secs(30),TerminalOptions::default().focus_return,clear_wait)
    }
    fn start_options(&self,events:broadcast::Sender<hangar_server::runtime::protocol::RuntimeEvent>,stall_notice:Duration,focus_return:Duration,clear_wait:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_plugin();
        let lease=crate::lease_when_free(&self.target.lease_path);
        let store=Store::open(&self.target.state_path,&self.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
        let options=TerminalOptions {interactions:Some(self.plugin.interactions().clone()),plugin:Some(self.plugin.clone()),io:self.io.clone(),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_millis(15),stall_notice,focus_return,clear_wait,status:Some(self.status.clone()),turn_start_wait:self.turn_start_wait(),..TerminalOptions::default()};
        TerminalActor::spawn(self.target.clone(),QueueActor::start(store,lease),self.policy.clone(),options,events,Arc::new(AtomicU64::new(0)))
    }
    fn start_give_up(&self,give_up_after:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_plugin();
        let lease=crate::lease_when_free(&self.target.lease_path);
        let store=Store::open(&self.target.state_path,&self.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
        let options=TerminalOptions {interactions:Some(self.plugin.interactions().clone()),plugin:Some(self.plugin.clone()),io:self.io.clone(),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_millis(15),give_up_after,turn_start_wait:self.turn_start_wait(),..TerminalOptions::default()};
        TerminalActor::spawn(self.target.clone(),QueueActor::start(store,lease),self.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)))
    }
    fn start_grace(&self,start_grace:Duration)->hangar_server::runtime::terminal::TerminalHandle {
        self.start_plugin();
        let lease=crate::lease_when_free(&self.target.lease_path);
        let store=Store::open(&self.target.state_path,&self.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
        let options=TerminalOptions {interactions:Some(self.plugin.interactions().clone()),plugin:Some(self.plugin.clone()),io:self.io.clone(),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_millis(15),status:Some(self.status.clone()),start_grace,turn_start_wait:self.turn_start_wait(),..TerminalOptions::default()};
        TerminalActor::spawn(self.target.clone(),QueueActor::start(store,lease),self.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)))
    }
    /// Espera do turno depois de uma entrega comum: zero por padrão, para os testes de duas mensagens seguidas.
    fn turn_start_wait(&self)->Duration {Duration::from_millis(self.turn_start_ms.load(std::sync::atomic::Ordering::Acquire))}
    fn command(&self,id:&str,text:&str)->RuntimeCommand { RuntimeCommand {operation_id:id.into(),kind:OperationKind::Input,payload:json!({"text":text,"pre_transcript":false})} }
    fn state(&self)->Value {
        serde_json::from_slice(&queue::read_replaced(&self.target.state_path).unwrap_or_else(|error|panic!("estado ilegível: {error}"))).unwrap()
    }
    fn sent_prompts(&self)->Vec<String> {
        self.io.calls.lock().unwrap().iter().filter_map(|call| {
            if call.args[0]=="load-buffer" {Some(String::from_utf8(call.stdin.clone()).unwrap())}
            else if call.args[0]=="send-keys" && call.args.contains(&"-l".into()) && call.args.last().is_some_and(|text|text!="\r") {call.args.last().cloned()}
            else {None}
        }).collect()
    }
    /// Espera com prazo; no estouro mostra onde a operação parou (multiplexador e diário).
    /// O teto cobre o runner Windows, onde cada leitura de fatos grava o diário durável em ~0,1–0,3 s.
    async fn wait_for(&self,what:&str,mut done:impl FnMut()->bool) {
        let start=std::time::Instant::now();
        while !done() {
            if start.elapsed()>WAIT {
                let io:Vec<String>=self.io.calls.lock().unwrap().iter().map(|r|r.args[0].clone()).collect();
                let ops:Vec<String>=self.state()["operations"].as_object().map(|o|o.iter().map(|(k,v)|format!("{k}={}",v["status"])).collect()).unwrap_or_default();
                panic!("{what}: prazo de {WAIT:?} estourou; io={io:?} operações={ops:?}");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    /// Quanto depois de `since` a operação entrou no diário (o `Prepare`, a primeira escrita dela). O
    /// comando guardado pela reserva só entra depois de solto; o resto da entrega, que no runner Windows
    /// grava o diário várias vezes e passa de segundos, fica fora da medida.
    async fn entered(&self,id:&str,since:std::time::Instant)->Duration {
        loop {
            if !self.state()["operations"][id].is_null() {return since.elapsed();}
            assert!(since.elapsed()<WAIT,"{id} não entrou no diário");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
impl Drop for Fixture {fn drop(&mut self){if let Some(task)=self.plugin_task.lock().unwrap().take() {task.abort();}}}

#[tokio::test]
async fn real_process_timeout_ends_grandchild_before_detach_and_new_lease() {
    let mut f=Fixture::new().await;let script=f._dir.path().join("fake_mux.py");let late=f._dir.path().join("late-write");
    let code=format!(r#"import os,sys,time,subprocess
path={}
# O pane é o próprio teste (o agente da sessão): no Windows o `python` do venv é um lançador, e o pai
# do script não seria o teste.
agent={}
mode=sys.argv[1]
if mode in ('--child','--grand'):
 open(path+mode+'.pid','w').write(str(os.getpid()))
 if mode=='--child':subprocess.Popen([sys.executable,__file__,'--grand'])
 else:time.sleep(2.5);open(path,'w').write('late')
 time.sleep(10)
elif mode=='--other':time.sleep(60)
elif mode=='display-message':print(str(agent)+'\tsession\t%1\t1' if sys.argv[-1].startswith('#{{pane_pid}}') else 'session\t%1\t1')
elif mode=='capture-pane':print('─'*32+'\n❯ \n'+'─'*32)
elif mode=='send-keys' and '-l' in sys.argv:
 subprocess.Popen([sys.executable,__file__,'--child'])
 # O prazo só pode vencer com o neto já nascido; senão o teste não prova nada.
 deadline=time.monotonic()+10
 while not os.path.exists(path+'--grand.pid') and time.monotonic()<deadline:time.sleep(.005)
 time.sleep(10)
"#,json!(late.to_str().unwrap()),std::process::id());
    std::fs::write(&script,code).unwrap();
    let python=std::env::var("HANGAR_TEST_PYTHON").unwrap_or_else(|_|if cfg!(windows){"python".into()}else{"python3".into()});
    let mut other=KillOnDrop(std::process::Command::new(&python).arg(&script).arg("--other").spawn().unwrap());let other_born=std::time::Instant::now();
    f.target.binding.mux_argv=vec![python,"-X".into(),"utf8".into(),script.to_str().unwrap().into()];
    let lease=crate::lease_when_free(&f.target.lease_path);let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let options=TerminalOptions {io:Arc::new(ProcessIo {command_timeout:Duration::from_millis(1500),socket_timeout:Duration::from_millis(150)}),
        limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1,..InputLimits::default()},tick:Duration::from_secs(10),stall_notice:Duration::from_secs(30),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    // Teto só contra travamento: o fluxo parte o multiplexador falso umas 16 vezes, e cada partida do
    // Python demora muito mais com a máquina carregada.
    let result=tokio::time::timeout(Duration::from_secs(30),h.command(f.command("timeout","A"))).await.unwrap().unwrap();
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    h.stop().await.unwrap();let python_lease=crate::lease_when_free(&f.target.lease_path);
    let grandchild_born=std::path::Path::new(&format!("{}--grand.pid",late.display())).exists();
    // O neto escreveria 2,5 s depois de nascer, e ele nasce antes do prazo de 1,5 s.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let old_writer=late.exists();let other_exit=other.0.try_wait().unwrap();let other_age=other_born.elapsed();let unrelated_alive=other_exit.is_none();
    for suffix in if old_writer {vec!["--child.pid","--grand.pid"]}else{vec![]} {
        if let Ok(pid)=std::fs::read_to_string(format!("{}{suffix}",late.display())) {
            #[cfg(unix)] {let _=std::process::Command::new("kill").args(["-9",pid.trim()]).output();}
            #[cfg(windows)] {let _=std::process::Command::new("taskkill.exe").args(["/PID",pid.trim(),"/T","/F"]).output();}
        }
    }
    drop(other);drop(python_lease);
    assert!(grandchild_born,"timeout fired before the grandchild existed; nothing was proved");
    // Saída 0 é o `sleep` do processo alheio que acabou sozinho; outro código é morte por terceiro.
    assert!(unrelated_alive,"processo alheio saiu: {other_exit:?} depois de {other_age:?}");assert!(!old_writer,"auxiliary grandchild wrote after detach released the lease");
}

/// Teto de chamadas ao multiplexador numa entrega de texto até a primeira tecla, pelo desenho: a vida
/// (pane e agente) é provada uma vez no começo da entrega (1); cada leitura de fatos relê a tela e
/// reconfere a vida (2), e são cinco: admissão do executor, prontidão do `prompt`, o composer, a do
/// `deliver` e a conferência final; o composer captura também a tela dele (1); a guarda de foco dos mods
/// confere o pane e lê o tamanho e a tela visível (3). 1 + 5×2 + 1 + 3 = 15.
const PROMPT_MUX_CEILING:usize=15;
#[tokio::test]
async fn terminal_runtime_prompt_reads_the_mux_once_per_stage_and_checks_life_last() {
    let f=Fixture::new().await;let h=f.start();f.io.calls.lock().unwrap().clear();
    assert_eq!(h.command(f.command("count","fixture-input")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let calls=f.io.calls.lock().unwrap().clone();
    let key=calls.iter().position(|c|c.args[0]=="send-keys").expect("a entrega não escreveu");
    let seen:Vec<String>=calls[..key].iter().map(|c|format!("{} {}",c.args[0],c.args.last().unwrap())).collect();
    assert!(key<=PROMPT_MUX_CEILING,"{key} chamadas antes da tecla: {seen:?}");
    // A conferência final relê a tela e só depois prova a vida, logo antes da tecla.
    assert_eq!(calls[key-2].args[0],"capture-pane","{seen:?}");
    assert_eq!(calls[key-1].args.last().unwrap(),"#{pane_pid}\t#{session_name}\t#{pane_id}\t#{session_created}","{seen:?}");
    h.stop().await.unwrap();
}
fn menu(title:&str,options:&[&str])->String {
    let lines:Vec<String>=options.iter().enumerate().map(|(i,o)|format!("{} {}. {o}",if i==0 {"❯"} else {" "},i+1)).collect();
    format!("{title}\n{}\nEsc to cancel · to navigate\n",lines.join("\n"))
}
/// Responde pelo ator ao menu `shown`, registrado como o monitor o publica, com `panes` na tela (uma por
/// captura, a última fica). Devolve a resposta e as teclas que saíram.
async fn answer_menu(kind:&str,option:u64,shown:&str,panes:Vec<String>)->(hangar_server::runtime::protocol::RuntimeReply,Vec<String>) {
    use hangar_server::interactions::{InteractionKind,InteractionTarget};
    let f=Fixture::new().await;let h=f.start();let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    *f.io.panes.lock().unwrap()=panes;
    let target=InteractionTarget {kind:InteractionKind::Menu,request_key:"terminal:menu".into()};
    let id=f.plugin.interactions().observe_revision(&binding,&target,Some(interaction_semantic(shown,None).unwrap()),None).unwrap().unwrap();
    f.io.calls.lock().unwrap().clear();
    let mut payload=json!({"interaction_id":id});if kind=="select" {payload["option"]=json!(option);}
    let reply=h.control(format!("{kind}-answer"),kind.into(),payload).await.unwrap();h.stop().await.unwrap();
    let keys=f.io.calls.lock().unwrap().iter().filter(|c|c.args[0]=="send-keys").map(|c|c.args.last().unwrap().clone()).collect();
    (reply,keys)
}
/// Menu A respondido e a tela passa a B em qualquer captura até a da conferência final da tecla (a 7ª: duas
/// por `check_interaction`, três vezes, e a do `verify` da tecla), depois de a vida já provada: recusa, sem tecla.
#[tokio::test]
async fn terminal_runtime_select_refuses_b_shown_at_any_capture_up_to_the_key() {
    let (a,b)=(menu("Question A",&["first","second"]),menu("Question B",&["first","second"]));
    for shown in 0..=6 {
        let mut panes=vec![a.clone();shown];panes.push(b.clone());
        let (reply,keys)=answer_menu("select",1,&a,panes).await;
        assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Rejected,"B na captura {}: {reply:?}",shown+1);
        assert_eq!(reply.payload["code"],"erro_interacao_desatualizada","{reply:?}");
        assert!(keys.is_empty(),"tecla com B na tela (captura {}): {keys:?}",shown+1);
    }
}
/// A navegação até a opção: B na conferência final da primeira seta não deixa tecla nenhuma sair; B na da
/// segunda deixa só a primeira: só setas saíram, sem Enter, então é recusa e não incerteza.
#[tokio::test]
async fn terminal_runtime_select_navigation_refuses_b_at_each_arrow() {
    let (a,b)=(menu("Question A",&["first","second","third"]),menu("Question B",&["first","second","third"]));
    // Capturas 1-4: os dois `check_interaction`; a 5ª é a da primeira seta, a 6ª a da segunda.
    let (reply,keys)=answer_menu("select",3,&a,[vec![a.clone();4],vec![b.clone()]].concat()).await;
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Rejected,json!("erro_interacao_desatualizada")),"{reply:?}");
    assert!(keys.is_empty(),"{keys:?}");
    let (reply,keys)=answer_menu("select",3,&a,[vec![a.clone();5],vec![b.clone()]].concat()).await;
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Rejected,json!("erro_interacao_desatualizada")),"{reply:?}");
    assert_eq!(keys,["Down"]);
}
/// Envio do menu de múltipla escolha: o `Right` só com o menu na conferência final; o Enter só com a mesma
/// aba de envio que o `Right` abriu.
#[tokio::test]
async fn terminal_runtime_submit_selected_refuses_changed_menu_or_review_before_each_key() {
    let (a,b)=(menu("Question A",&["[✔] first","[ ] second"]),menu("Question B",&["[✔] first","[ ] second"]));
    let review=|answer:&str|format!("Review your answers\n → {answer}\n\n ❯ 1. Submit answers\n   2. Cancel\n");
    // Capturas 1-4: `check_interaction` e a leitura do envio; a 5ª é a do `Right`.
    let (reply,keys)=answer_menu("submit_selected",0,&a,[vec![a.clone();4],vec![b.clone()]].concat()).await;
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Rejected,json!("erro_interacao_desatualizada")),"{reply:?}");
    assert!(keys.is_empty(),"{keys:?}");
    // Depois do `Right`, a 6ª e a 7ª mostram a aba de envio; a 8ª, a do Enter, outra.
    let (reply,keys)=answer_menu("submit_selected",0,&a,[vec![a.clone();5],vec![review("first");2],vec![review("second")]].concat()).await;
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Unknown,json!("erro_interacao_desatualizada")),"{reply:?}");
    assert_eq!(keys,["Right"]);
    // Sem troca, o Enter sai para a mesma aba.
    let (reply,keys)=answer_menu("submit_selected",0,&a,[vec![a.clone();5],vec![review("first");3],vec!["❯ \n".into()]].concat()).await;
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{reply:?}");
    assert_eq!(keys,["Right","\r"]);
}
/// A conversa (vida) ou o pane trocam em qualquer chamada até a última antes da tecla: a entrega não escreve.
#[tokio::test]
async fn terminal_runtime_prompt_refuses_life_or_pane_switch_at_any_call_before_the_key() {
    let clean={
        let f=Fixture::new().await;let h=f.start();f.io.calls.lock().unwrap().clear();
        assert_eq!(h.command(f.command("clean","fixture-input")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
        let key=f.io.calls.lock().unwrap().iter().position(|c|c.args[0]=="send-keys").unwrap();h.stop().await.unwrap();key
    };
    for kind in ["life","pane"] {
        for at in 1..=clean {
            let f=Fixture::new().await;let h=f.start();f.io.calls.lock().unwrap().clear();*f.io.switch_at.lock().unwrap()=Some((at,kind));
            let reply=h.command(f.command("switched","fixture-input")).await.unwrap();
            assert_ne!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{kind} trocado na chamada {at}");
            h.stop().await.unwrap();
            assert!(f.io.calls.lock().unwrap().iter().all(|c|c.args[0]!="send-keys"),"{kind} trocado na chamada {at}: tecla escrita");
        }
    }
}

#[cfg(target_os="linux")]
#[tokio::test]
async fn command_leader_stays_unreaped_until_its_group_is_gone() {
    // O neto vigia o líder: se o número dele some enquanto o grupo ainda vive, outro processo
    // poderia herdá-lo e levar o SIGKILL do grupo.
    let dir=tempfile::tempdir().unwrap();let script=dir.path().join("leader.py");let base=dir.path().join("probe");
    let code=format!(r#"import os,sys,time,subprocess
base={}
if sys.argv[1]=='--grand':
 leader=sys.argv[2];open(base+".ready","w").write("1")
 while True:
  if not os.path.exists('/proc/'+leader):open(base+'.reaped','w').write('1');break
  time.sleep(.0002)
 time.sleep(10)
else:
 subprocess.Popen([sys.executable,__file__,'--grand',str(os.getpid())],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL)
 deadline=time.monotonic()+5
 while not os.path.exists(base+'.ready') and time.monotonic()<deadline:time.sleep(.005)
 print('leader-done')
"#,json!(base.to_str().unwrap()));
    std::fs::write(&script,code).unwrap();
    let python=std::env::var("HANGAR_TEST_PYTHON").unwrap_or_else(|_|"python3".into());
    let io=ProcessIo {command_timeout:Duration::from_secs(10),socket_timeout:Duration::from_secs(1)};
    for _ in 0..5 {
        for suffix in [".ready",".reaped"] {let _=std::fs::remove_file(format!("{}{suffix}",base.display()));}
        let output=io.command(CommandRequest {program:python.clone(),args:vec![script.to_str().unwrap().into(),"--leader".into()],stdin:vec![]}).await.unwrap();
        assert!(output.success);assert_eq!(String::from_utf8_lossy(&output.stdout).trim(),"leader-done");
        assert!(std::path::Path::new(&format!("{}.ready",base.display())).exists(),"grandchild never started");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let reaped=std::path::Path::new(&format!("{}.reaped",base.display())).exists();
        assert!(!reaped,"leader was reaped while its group was still alive");
    }
}

#[tokio::test]
async fn terminal_v2_prepare_confirmed_has_zero_policy_or_key() {
    let f=Fixture::new().await;
    let initial=queue::State::new("key",1,"session",vec![json!({"id":"root","text":"fixture-input","ts":1.0,"delivered":true,"confirmed":true})]);
    std::fs::write(&f.target.state_path,serde_json::to_vec(&initial).unwrap()).unwrap();
    let h=f.start();let result=h.command(f.command("root","fixture-input")).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    assert!(f.io.calls.lock().unwrap().is_empty());
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_v2_sequence_uses_durable_watermark() {
    let f=Fixture::new().await;let mut initial=queue::State::new("key",1,"session",vec![]);initial.next_seq=50_000;
    std::fs::write(&f.target.state_path,serde_json::to_vec(&initial).unwrap()).unwrap();
    let h=f.start();h.command(f.command("root","fixture-input")).await.unwrap();
    let state=f.state();
    assert!(state["operations"].as_object().unwrap().keys().filter(|id|id.starts_with("call::terminal:")||id.starts_with("terminal-policy:"))
        .all(|id|id.rsplit(':').next().unwrap().parse::<u64>().unwrap()>=50_000));
    h.stop().await.unwrap();
}

#[tokio::test]
async fn unknown_fill_blocks_second_input_after_detach_restart_and_same_sid_generation() {
    let mut f=Fixture::new().await;f.unknown.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start();assert_eq!(h.command(f.command("A","A")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    *f.io.text.lock().unwrap()="A".into();
    h.queue("append-B".into(),Action::Append {text:"B".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("B".into())}).await.unwrap();
    h.stop().await.unwrap();
    let mut state=f.state();state["generation"]=json!(2);
    std::fs::write(&f.target.state_path,serde_json::to_vec(&state).unwrap()).unwrap();
    f.target.generation=2;f.target.binding.generation=2;f.generation.store(2,std::sync::atomic::Ordering::Release);
    let effects=f.io.calls.lock().unwrap().len();
    f.unknown.store(false,std::sync::atomic::Ordering::Release);
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",2,"session",vec![])).unwrap();
    let options=TerminalOptions {io:f.io.clone(),limits:InputLimits::default(),tick:Duration::from_millis(15),stall_notice:Duration::from_secs(30),status:Some(f.status.clone()),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    assert_eq!(h.snapshot().await.unwrap()["view"]["input_stalled"], "delivery_unproved");
    let late=f.status.subscribe();
    assert_eq!(late.borrow().input_stalled.as_deref(), Some("delivery_unproved"), "a reabertura publica o bloqueio antes de atender o monitor");
    assert_eq!(h.drain().await.unwrap()["drained"],0);
    assert_eq!(h.command(f.command("C","C")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(f.io.calls.lock().unwrap().len(),effects);
    assert_eq!(*f.io.text.lock().unwrap(),"A");h.stop().await.unwrap();
}

#[tokio::test]
async fn clear_with_arguments_passes_terminal_write_barrier_like_python() {
    for text in ["/clear","  /clear keep"] {
        let f=Fixture::new().await;f.unknown.store(true,std::sync::atomic::Ordering::Release);
        let h=f.start();assert_eq!(h.command(f.command("A","A")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
        f.unknown.store(false,std::sync::atomic::Ordering::Release);
        let result=h.command(f.command("clear",text)).await.unwrap();
        assert_ne!(result.payload["code"],"terminal_write_barrier","{text}");
        h.stop().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_runtime_maintenance_failure_publishes_problem_and_stops_uncoordinated_retry() {
    let f=Fixture::new().await;
    let (events,mut receiver)=broadcast::channel(128); let h=f.start_with_events(events);
    h.command(f.command("accepted","Olá")).await.unwrap();
    std::fs::remove_file(&f.target.transcript).unwrap();
    std::fs::create_dir(&f.target.transcript).unwrap();
    let problem=tokio::time::timeout(WAIT,async {
        loop {let event=receiver.recv().await.unwrap();if event.channel=="problem"{break event;}}
    }).await.unwrap();
    assert_eq!(problem.data["error_code"],"receipt_scan");
    assert_eq!(f.status.borrow().error.as_deref(), Some("receipt_scan"));
    assert_eq!(f.status.borrow().message.as_deref(), problem.data["message"].as_str());
    assert_eq!(h.snapshot().await.unwrap()["error"],"receipt_scan");
    let count=f.state()["operations"].as_object().unwrap().len();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(f.state()["operations"].as_object().unwrap().len(),count);
    std::fs::remove_dir(&f.target.transcript).unwrap();
    std::fs::write(&f.target.transcript,"").unwrap();
    h.confirm().await.unwrap();
    assert!(h.snapshot().await.unwrap()["error"].is_null());
    assert_eq!(*f.status.borrow(), TerminalStatus::default());
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_status_reopening_resets_old_error_before_subscribing() {
    let f=Fixture::new().await;
    for _ in 0..2 {
        f.status.send_replace(TerminalStatus {error:Some("old_error".into()),message:Some("erro da vida anterior".into()),input_stalled:None,limit:None});
        let h=f.start();
        h.snapshot().await.unwrap();
        let late=f.status.subscribe();
        assert_eq!(*late.borrow(), TerminalStatus::default());
        h.stop().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_runtime_unknown_delivery_is_signaled_without_retyping() {
    let f=Fixture::new().await; f.unknown.store(true,std::sync::atomic::Ordering::Release);
    let (events,mut receiver)=broadcast::channel(128); let h=f.start_with_events(events);
    let command=f.command("unknown","Olá");
    assert_eq!(h.command(command.clone()).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    let problem=tokio::time::timeout(WAIT,async {
        loop {let event=receiver.recv().await.unwrap();if event.channel=="problem"{break event;}}
    }).await.unwrap();
    assert_eq!(problem.data["error_code"],"terminal_delivery_unknown");
    assert_eq!(f.status.borrow().error.as_deref(), Some("terminal_delivery_unknown"));
    assert_eq!(h.snapshot().await.unwrap()["error"],"terminal_delivery_unknown");
    let calls=f.io.calls.lock().unwrap().len();
    h.command(command).await.unwrap(); tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(f.io.calls.lock().unwrap().len(),calls);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_unknown_delivery_seen_in_transcript_clears_error_once() {
    let f=Fixture::new().await; f.unknown.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    assert_eq!(h.command(f.command("late","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    assert_eq!(h.snapshot().await.unwrap()["error"],"terminal_delivery_unknown");
    std::fs::write(&f.target.transcript,"{\"type\":\"user\",\"uuid\":\"1\",\"message\":{\"content\":\"Olá\"}}\n").unwrap();
    f.wait_for("reconciliada",||f.state()["rows"][0]["confirmed"]==true).await;
    let start=std::time::Instant::now();
    while !h.snapshot().await.unwrap()["error"].is_null() {assert!(start.elapsed()<WAIT,"erro não limpou"); tokio::time::sleep(Duration::from_millis(5)).await;}
    assert_eq!(f.state()["operations"]["late"]["status"],"confirmed");
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada foi redigitado");
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_repeat_id_does_not_write_twice_and_conflicting_payload_fails() {
    let f=Fixture::new().await; let h=f.start(); let command=f.command("first","Olá");
    assert_eq!(h.command(command.clone()).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let count=f.io.calls.lock().unwrap().len();
    h.command(command).await.unwrap(); assert_eq!(f.io.calls.lock().unwrap().len(),count);
    assert!(h.command(f.command("first","Outro")).await.is_err()); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_unknown_survives_restart_and_never_requeues() {
    let f=Fixture::new().await; f.unknown.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let command=f.command("uncertain","Olá"); assert_eq!(h.command(command.clone()).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    h.stop().await.unwrap(); let h=f.start(); h.command(command).await.unwrap(); h.drain().await.unwrap();
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"));
    assert_eq!(f.state()["rows"][0]["delivered"],true); h.stop().await.unwrap();
}
#[tokio::test]
async fn queued_batch_terminal_sends_one_prompt_and_confirms_all_members() {
    let f=Fixture::new().await;
    f.turn_start_ms.store(60_000,std::sync::atomic::Ordering::Release);
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    let h=f.start();
    for (id,text) in [("a","primeira"),("b","segunda"),("c","terceira")] {
        assert_eq!(h.command(f.command(id,text)).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    }
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("o driver escreveu",||f.io.calls.lock().unwrap().iter().any(|call|call.args[0]=="send-keys" && call.args.last().is_some_and(|key|key=="\r"))).await;
    let sent=f.sent_prompts();
    assert_eq!(sent,vec!["primeira\nsegunda\nterceira"],"o driver recebe um único prompt inteiro");
    let state=f.state();assert_eq!(state["rows"].as_array().unwrap().len(),1);
    assert_eq!(state["rows"][0]["batch_entries"].as_array().unwrap().len(),3);
    std::fs::write(&f.target.transcript,format!("{}\n",json!({"type":"user","uuid":"batch-echo","sessionId":"sid","message":{"role":"user","content":"primeira\nsegunda\nterceira"}}))).unwrap();
    h.confirm().await.unwrap();
    assert!(f.state()["rows"][0]["batch_entries"].as_array().unwrap().iter().all(|row|row["confirmed"]==true));
    h.stop().await.unwrap();
}

#[tokio::test]
async fn queued_batch_terminal_reservation_is_internal() {
    let f=Fixture::new().await;let h=f.start();
    assert_eq!(h.queue("outside-claim".into(),Action::ClaimBatch {min_ts:0.0}).await.unwrap_err().code,"terminal_queue_action");
    h.stop().await.unwrap();
}

#[tokio::test]
async fn queued_batch_terminal_oversized_batch_stays_editable() {
    let f=Fixture::new().await;f.idle.store(false,std::sync::atomic::Ordering::Release);
    let rows=["a","b"].into_iter().map(|id|json!({"id":id,"text":"x".repeat(hangar_server::runtime::protocol::MAX_FRAME/2),"ts":2.0,"delivered":false})).collect();
    drop(Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",rows)).unwrap());
    let h=f.start();
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    let snapshot=tokio::time::timeout(Duration::from_secs(60),async {loop {
        let snapshot=h.snapshot().await.unwrap();
        if !snapshot["error"].is_null() || snapshot["view"]["input_stalled"]=="queue_batch_too_large" {break snapshot;}
        tokio::task::yield_now().await;
    }}).await.unwrap();
    assert!(snapshot["error"].is_null(),"o tamanho do lote não adoece a entrada: {}",snapshot["error"]);
    assert_eq!(snapshot["view"]["input_stalled"],"queue_batch_too_large");
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    assert_eq!(h.queue("edit-a".into(),Action::Edit {entry_id:"a".into(),text:"curta".into()}).await.unwrap(),"edited");
    h.queue("edit-b".into(),Action::Edit {entry_id:"b".into(),text:"outra".into()}).await.unwrap();
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("o lote reduzido saiu",||f.io.calls.lock().unwrap().iter().any(|call|call.args[0]=="load-buffer" && call.stdin==b"curta\noutra")).await;
    assert!(h.snapshot().await.unwrap()["error"].is_null());
    h.stop().await.unwrap();
}

#[tokio::test]
async fn queued_batch_terminal_size_block_rechecks_after_internal_queue_changes() {
    for steer in [false,true] {
        let f=Fixture::new().await;f.start_plugin();
        let large="\"".repeat((hangar_server::runtime::protocol::MAX_FRAME-100)/4);
        let small="\"".repeat(100);
        let rows=vec![json!({"id":"small","text":small,"ts":2.0,"delivered":false}),
            json!({"id":"b","text":large,"ts":2.0,"delivered":false}),json!({"id":"c","text":large,"ts":2.0,"delivered":false})];
        let lease=crate::lease_when_free(&f.target.lease_path);
        let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",rows)).unwrap();
        let options=TerminalOptions {io:f.io.clone(),plugin:Some(f.plugin.clone()),interactions:Some(f.plugin.interactions().clone()),
            tick:Duration::from_secs(600),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},turn_start_wait:Duration::ZERO,..TerminalOptions::default()};
        let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
        h.drain().await.unwrap();
        assert_eq!(h.snapshot().await.unwrap()["view"]["input_stalled"],"queue_batch_too_large");
        if steer {
            let result=h.control("steer-small".into(),"steer_entry".into(),json!({"entry_id":"small"})).await.unwrap();
            assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Accepted);
            let remaining=format!("{large}\n{large}");
            assert!(json!({"modo":"fill","text":remaining}).to_string().len()<hangar_server::runtime::protocol::MAX_FRAME);
            // Para na leitura seguinte: prova a retentativa sem renderizar megabytes no dublê do pane.
            f.io.capture_error.store(true,std::sync::atomic::Ordering::Release);
            assert_eq!(h.drain().await.unwrap_err().code,"terminal_facts","o bloqueio antigo não impede reavaliar a entrega");
            assert_eq!(f.sent_prompts(),vec![small.clone()]);
        }else{
            assert_eq!(h.command(f.command("new","nova mensagem")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
            assert!(h.snapshot().await.unwrap()["view"]["input_stalled"].is_null(),"a escrita interna invalida a avaliação anterior");
        }
        h.stop().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_runtime_idle_timer_drains_without_sse_as_one_batch() {
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("queued1","Um")).await.unwrap(); h.command(f.command("queued2","Dois")).await.unwrap();
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));
    f.idle.store(true,std::sync::atomic::Ordering::Release); f.ready.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("espera 1",||f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==true)).await;
    let state=f.state(); assert_eq!(state["rows"].as_array().unwrap().len(),1);assert_eq!(state["rows"][0]["text"],"Um\nDois"); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_next_batch_waits_for_the_next_idle_turn() {
    // A espera do turno cobre o intervalo entre a colagem do lote e o registro nativo passar a ocupado.
    let f=Fixture::new().await; f.turn_start_ms.store(60_000,std::sync::atomic::Ordering::Release);
    f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    let reply=h.command(f.command("one","Um")).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Deferred,json!("terminal_turn_hold")),"{}",reply.payload);
    h.command(f.command("two","Dois")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"),"nada digitado durante o turno");
    assert!(f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==false));
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("o lote saiu parado",||f.io.calls.lock().unwrap().iter().any(|call|call.args[0]=="send-keys" && call.args.last().is_some_and(|text|text=="\r"))).await;
    assert_eq!(f.state()["rows"][0]["text"],"Um\nDois");
    // O registro ainda diz parado: a próxima mensagem não cai no mesmo turno.
    assert_eq!(h.command(f.command("three","Três")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred,"o /input novo também espera");
    // Ocupado até a drenagem ler de fato: cada leitura de fatos captura o pane, e o registro nativo é regravado
    // com `busy` na primeira chamada depois da troca. Três capturas depois dela incluem uma leitura inteira já
    // ocupada; o segundo mínimo cobre o runner lento, onde cada leitura leva décimos de segundo.
    let captures=||f.io.calls.lock().unwrap().iter().filter(|c|c.args[0]=="capture-pane").count();
    let before=captures(); let busy_since=std::time::Instant::now();
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    f.wait_for("a drenagem leu a sessão ocupada",||captures()>=before+3 && busy_since.elapsed()>=Duration::from_secs(1)).await;
    assert_eq!(f.state()["rows"][1]["delivered"],false);
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("o próximo lote saiu depois",||f.state()["rows"][1]["delivered"]==true).await;
    assert_eq!(f.state()["rows"].as_array().unwrap().len(),2);
    assert_eq!(f.state()["rows"][1]["text"],"Três");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_direct_input_right_after_a_delivery_waits_for_the_turn() {
    // A entrega direta com o Claude parado abre um turno; o /input seguinte, antes de ele aparecer ocupado, espera.
    let f=Fixture::new().await; f.turn_start_ms.store(60_000,std::sync::atomic::Ordering::Release); let h=f.start();
    assert_eq!(h.command(f.command("first","Um")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let reply=h.command(f.command("second","Dois")).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Deferred,json!("terminal_turn_hold")),"{}",reply.payload);
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_slash_command_is_refused_while_messages_wait() {
    // Um `/clear` no meio do turno apagaria as mensagens seguradas (a troca de conversa esvazia a fila).
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap();
    for (id,text) in [("clear","/clear"),("help","/help")] {
        let reply=h.command(f.command(id,text)).await.unwrap();
        assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Deferred,json!("queue_waiting")),"{}",reply.payload);
    }
    assert!(f.io.calls.lock().unwrap().iter().all(|c|c.args[0]!="send-keys"),"nada digitado");
    assert!(f.state()["runtime_state"]["clear_barrier"].is_null());
    assert_eq!(f.state()["rows"].as_array().unwrap().iter().filter(|r|r["delivered"]==false).count(),1,"a mensagem segue esperando");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_native_message_is_not_held_by_the_turn() {
    // Recado de outra sessão pelo socket nativo: é o canal do próprio Claude Code para o meio do turno.
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); f.native.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let reply=h.command(f.command("peer","[de: peer] Olá")).await.unwrap();
    assert_ne!(reply.payload["code"],"terminal_turn_hold","{}",reply.payload);
    assert_eq!(f.io.socket_calls.lock().unwrap().len(),1);
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_plain_text_is_held_even_with_the_native_socket() {
    // O socket nativo só leva recado `[de: …]`; a fala da pessoa espera o turno como sempre.
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); f.native.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let reply=h.command(f.command("mine","Olá")).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Deferred,json!("terminal_turn_hold")),"{}",reply.payload);
    assert!(f.io.socket_calls.lock().unwrap().is_empty());
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_native_message_behind_held_rows_still_goes_mid_turn() {
    // Recado que chegou atrás de uma fala segurada: a drenagem o leva pelo socket sem esperar o turno.
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); f.native.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("mine","Olá")).await.unwrap();
    h.command(f.command("peer","[de: peer] Oi")).await.unwrap();
    f.wait_for("recado saiu pelo socket",||f.io.socket_calls.lock().unwrap().len()==1).await;
    assert_eq!(f.state()["rows"].as_array().unwrap().iter().find(|r|r["id"]=="mine").unwrap()["delivered"],false,"a fala segue segurada");
    assert!(f.io.calls.lock().unwrap().iter().all(|c|c.args[0]!="send-keys"),"nada digitado durante o turno");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_entry_types_and_promotes_mid_turn() {
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap(); h.command(f.command("two","Dois")).await.unwrap();
    f.io.send_now_hint.store(true,std::sync::atomic::Ordering::Release);
    let reply=h.control("steer-two".into(),"steer_entry".into(),json!({"entry_id":"two"})).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{}",reply.payload);
    assert_eq!(reply.payload["steered"],true);
    let calls=f.io.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c|c.args.contains(&"-l".into()) && c.args.last().is_some_and(|t|t=="Dois")),"a escolhida foi digitada");
    assert!(calls.iter().any(|c|c.args.last().is_some_and(|k|k=="C-s")),"ctrl+x ctrl+s promoveu");
    assert_eq!(f.state()["rows"].as_array().unwrap().iter().find(|r|r["id"]=="one").unwrap()["delivered"],false,"a outra segue esperando");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_entry_waits_for_the_late_send_now_hint() {
    // O Claude Code só mostra a dica depois dos hooks de UserPromptSubmit da conta, mais de 1 s depois do Enter.
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap(); h.command(f.command("two","Dois")).await.unwrap();
    let io=f.io.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(900)).await; io.send_now_hint.store(true,std::sync::atomic::Ordering::Release); });
    let reply=h.control("steer-late".into(),"steer_entry".into(),json!({"entry_id":"two"})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["steered"].clone()),(hangar_server::runtime::protocol::Disposition::Accepted,json!(true)),"{}",reply.payload);
    assert!(f.io.calls.lock().unwrap().iter().any(|c|c.args.last().is_some_and(|k|k=="C-s")),"ctrl+x ctrl+s saiu depois da dica");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_entry_without_hint_says_so() {
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap(); h.command(f.command("two","Dois")).await.unwrap();
    let reply=h.control("steer-none".into(),"steer_entry".into(),json!({"entry_id":"two"})).await.unwrap();
    assert_eq!((reply.payload["steered"].clone(),reply.payload["code"].clone()),(json!(false),json!("queue_steer_hint_missing")),"{}",reply.payload);
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_entry_when_idle_is_a_plain_delivery() {
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap();
    // Parado, mas com a caixa ilegível: o tique não a leva antes do pedido, e o Orientar a deixa na frente.
    f.ready.store(false,std::sync::atomic::Ordering::Release); f.idle.store(true,std::sync::atomic::Ordering::Release);
    let reply=h.control("steer-one".into(),"steer_entry".into(),json!({"entry_id":"one"})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["steered"].clone()),(hangar_server::runtime::protocol::Disposition::Accepted,json!(false)));
    assert!(f.io.calls.lock().unwrap().iter().all(|c|!c.args.last().is_some_and(|k|k=="C-s")),"parado não promove");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_without_body_steers_the_first_waiting_entry() {
    // O chip de app antigo (`/steer` sem corpo) com mensagens esperando no Hangar: orienta a primeira.
    let f=Fixture::new().await; f.idle.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("one","Um")).await.unwrap(); h.command(f.command("two","Dois")).await.unwrap();
    f.io.send_now_hint.store(true,std::sync::atomic::Ordering::Release);
    let reply=h.control("steer-old".into(),"steer_first".into(),json!({})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["entry_id"].clone(),reply.payload["steered"].clone()),
        (hangar_server::runtime::protocol::Disposition::Accepted,json!("one"),json!(true)),"{}",reply.payload);
    let calls=f.io.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c|c.args.contains(&"-l".into()) && c.args.last().is_some_and(|t|t=="Um")),"a primeira foi digitada");
    assert_eq!(f.state()["rows"].as_array().unwrap().iter().find(|r|r["id"]=="two").unwrap()["delivered"],false,"a outra segue esperando");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_without_body_and_empty_queue_promotes_as_before() {
    let f=Fixture::new().await; let h=f.start();
    let reply=h.control("steer-old".into(),"steer_first".into(),json!({})).await.unwrap();
    assert!(reply.payload.get("entry_id").is_none(),"sem fila esperando, o caminho de antes: {}",reply.payload);
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_steer_entry_of_a_gone_entry_is_refused_without_keys() {
    let f=Fixture::new().await; let h=f.start();
    let reply=h.control("steer-x".into(),"steer_entry".into(),json!({"entry_id":"nada"})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Rejected,json!("queue_entry_gone")));
    assert!(f.io.calls.lock().unwrap().iter().all(|c|c.args[0]!="send-keys"));
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_identical_text_uses_distinct_occurrences_and_enqueue_stays_visible() {
    let f=Fixture::new().await; let h=f.start(); h.command(f.command("one","Olá — 📎 imagem: /tmp/x.png")).await.unwrap(); h.command(f.command("two","Olá — 📎 imagem: /tmp/x.png")).await.unwrap();
    std::fs::write(&f.target.transcript,"{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"Olá\"}\n").unwrap(); h.confirm().await.unwrap();
    assert!(f.state()["rows"].as_array().unwrap().iter().all(|r|r["confirmed"]!=true));
    std::fs::write(&f.target.transcript,"{\"type\":\"user\",\"uuid\":\"1\",\"message\":{\"content\":\"Olá\"}}\n").unwrap(); h.confirm().await.unwrap();
    assert_eq!(f.state()["rows"].as_array().unwrap().iter().filter(|r|r["confirmed"]==true).count(),1);
    use std::io::Write; let mut file=std::fs::OpenOptions::new().append(true).open(&f.target.transcript).unwrap(); writeln!(file,"{{\"type\":\"attachment\",\"uuid\":\"2\",\"attachment\":{{\"type\":\"queued_command\",\"prompt\":\"Olá\"}}}}").unwrap(); h.confirm().await.unwrap();
    assert!(f.state()["rows"].as_array().unwrap().iter().all(|r|r["confirmed"]==true)); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_cancel_http_does_not_release_lease_and_stop_waits() {
    let f=Fixture::new().await; f.io.blocked.store(true,std::sync::atomic::Ordering::Release); let h=f.start(); let hc=h.clone(); let cmd=f.command("flight","Olá");
    let request=tokio::spawn(async move {hc.command(cmd).await});
    f.wait_for("espera 2",||f.io.calls.lock().unwrap().iter().any(|r|r.args[0]=="send-keys")).await;
    request.abort(); let hc=h.clone(); let stopping=tokio::spawn(async move {hc.stop().await}); tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!stopping.is_finished()); assert!(queue::acquire_lease(&f.target.lease_path).is_err());
    f.io.blocked.store(false,std::sync::atomic::Ordering::Release); f.io.gate.notify_waiters(); stopping.await.unwrap().unwrap(); drop(crate::lease_when_free(&f.target.lease_path));
}
#[tokio::test]
async fn terminal_runtime_local_publication_keeps_journal_and_root_without_retired_policies() {
    let f=Fixture::new().await;f.unknown.store(true,std::sync::atomic::Ordering::Release);let h=f.start();
    let reply=h.command(f.command("root","Olá")).await.unwrap();assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    let requests=f.plugin_requests.lock().unwrap().clone();assert_eq!(requests.len(),1);
    assert_eq!(requests[0]["publication_id"],"key:1:root");assert_eq!(requests[0]["_writing"],"dispatching");assert_eq!(f.state()["operations"]["root"]["status"],"unknown");
    assert!(f.state()["operations"].as_object().unwrap().keys().all(|id|!id.starts_with("terminal-policy:")));h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_slash_has_no_prompt_row_and_private_snapshot_has_no_public_state() {
    let f=Fixture::new().await; let h=f.start(); h.command(f.command("slash","/help")).await.unwrap(); let s=h.snapshot().await.unwrap();
    assert_eq!(s["view"]["terminal"],true); assert!(s["view"].get("public_state").is_none()); assert!(f.state()["rows"].as_array().unwrap().is_empty()); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_first_prompt_missing_transcript_confirms_without_redelivery() {
    let f=Fixture::new().await; std::fs::remove_file(&f.target.transcript).unwrap(); let h=f.start(); h.command(f.command("first","Olá")).await.unwrap();
    std::fs::write(&f.target.transcript,format!("{}\n",json!({"type":"user","sessionId":"sid","uuid":"first","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"Olá"}}))).unwrap();
    h.confirm().await.unwrap(); assert_eq!(f.state()["rows"][0]["confirmed"],true); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_explicit_steer_uses_driver_and_no_prompt_row() {
    let f=Fixture::new().await; let h=f.start();
    let result=h.control("steer-now".into(),"steer".into(),json!({})).await.unwrap();
    // Sem a marca da fila na TUI não se prova consumo: adiado com o código.
    assert_eq!((result.disposition,result.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Deferred,json!("no_queued_input")),"{}",result.payload);
    assert!(f.state()["rows"].as_array().unwrap().is_empty()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_private_controls_validate_entire_payload_before_effect() {
    let f=Fixture::new().await; let h=f.start();
    for (kind,payload) in [("navigation_key",json!({"key":"C-c"})),("interactive_key",json!({"key":"-N"})),("terminal_input",json!({"text":"bad\u{1b}"})),("select",json!({"option":1,"bad":true})),("answer_questions",json!({"answers":[{"kind":"text","type_index":1,"value":"bad\u{1b}"}]}))] {
        let result=h.control(format!("bad-{kind}"),kind.into(),payload).await;
        assert!(result.is_err() || result.unwrap().disposition==hangar_server::runtime::protocol::Disposition::Rejected);
    }
    assert!(f.io.calls.lock().unwrap().is_empty()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_cleanup_proved_uses_same_queue_budget_original_plus_two() {
    let f=Fixture::new().await; f.io.fail_write.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let result=h.command(f.command("retry","Olá")).await.unwrap(); assert_eq!(result.payload["cleanup"],"proved");
    f.wait_for("espera 3",||f.state()["rows"][0]["desistiu"]==true).await;
    assert_eq!(f.state()["rows"][0]["attempts"],2);
    assert_eq!(f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys" && r.args.last().unwrap()=="Olá").count(),3);
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().is_some_and(|s|s=="\r")));
    h.command(f.command("retry","Olá")).await.unwrap(); h.drain().await.unwrap();
    assert_eq!(f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys" && r.args.last().unwrap()=="Olá").count(),3); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_dispatching_recovers_unknown_not_redigitated() {
    let f=Fixture::new().await;
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64};
    let command=f.command("crash","Olá");
    store.exec(1,"prepare",clock,Action::Prepare {id:"crash".into(),payload:serde_json::to_value(&command).unwrap(),entry_id:Some("crash".into())}).unwrap();
    store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("crash".into())}).unwrap();
    store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"crash".into(),wire_id:"attempt".into(),staged:false}).unwrap(); drop(store);
    let h=f.start(); assert_eq!(h.command(command).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    h.drain().await.unwrap(); assert!(f.io.calls.lock().unwrap().is_empty()); assert_eq!(f.state()["operations"]["crash"]["status"],"unknown");
    assert!(h.queue("unclaim".into(),Action::SetDelivered {entry_id:"crash".into(),value:false,steered:false}).await.is_err()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_held_plugin_unknown_does_not_fall_back_and_repeat_does_not_publish() {
    use hangar_server::plugin::{Ask,PermissionGate,Receipt};
    let f=Fixture::new().await;let h=f.start();f.owner(true);
    let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    let poll=f.plugin.begin_ask(&binding,Ask {id:"perm:held".into(),questions:None,tool:Some("Bash".into()),janela_ms:None,summary:Some("comando".into())},PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false},std::time::Instant::now()).unwrap();
    let interaction=f.plugin.pending(&binding,std::time::Instant::now()).unwrap().interaction_id;
    let payload=json!({"option":1,"request_id":"perm:held","interaction_id":interaction});let hc=h.clone();let sent=payload.clone();
    let command=tokio::spawn(async move {hc.control("held".into(),"select".into(),sent).await});
    let body=tokio::time::timeout(WAIT,poll.wait()).await.unwrap();assert_eq!(body["permitir"],true);assert!(body.get("answers").is_none());
    let receipt:Receipt=serde_json::from_value(json!({"publication_id":body["publication_id"],"generation":body["generation"],"session_id":body["session_id"]})).unwrap();
    f.plugin.finish(&binding,"perm:held","terminal",Some(receipt)).unwrap();
    assert_eq!(command.await.unwrap().unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    h.control("held".into(),"select".into(),payload).await.unwrap();
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_permission_without_receipt_never_gets_a_keyboard_fallback() {
    use hangar_server::plugin::{Ask,PermissionGate};
    let f=Fixture::new().await;let h=f.start();f.owner(false);let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    let poll=f.plugin.begin_ask(&binding,Ask {id:"perm:held".into(),questions:None,tool:None,janela_ms:None,summary:None},PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false},std::time::Instant::now()).unwrap();
    let interaction=f.plugin.pending(&binding,std::time::Instant::now()).unwrap().interaction_id;
    assert_eq!(h.control("select".into(),"select".into(),json!({"option":1,"request_id":"perm:held","interaction_id":interaction})).await.unwrap_err().code,"erro_interacao_indisponivel");
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]=="display-message"));
    assert!(f.plugin_requests.lock().unwrap().is_empty());
    assert!(tokio::time::timeout(Duration::ZERO,poll.wait()).await.is_err(),"nenhuma resposta entregue ao hook sem receipt_v2");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_question_without_receipt_falls_back_to_the_pane_keys() {
    // Vida nova antes do `/pull` voltar: o plugin segura a pergunta e não recebe a resposta. Como no Python, as
    // teclas do pane respondem, conferindo que o picker mostra a pergunta dele.
    use hangar_server::plugin::{Ask,PermissionGate};
    let f=Fixture::new().await;let h=f.start();f.owner(false);let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    let questions=vec![json!({"question":"Escolha B","header":"B","options":[{"label":"primeira"},{"label":"segunda"}]})];
    let poll=f.plugin.begin_ask(&binding,Ask {id:"ask:B".into(),questions:Some(questions),tool:None,janela_ms:None,summary:None},PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false},std::time::Instant::now()).unwrap();
    let interaction=f.plugin.pending(&binding,std::time::Instant::now()).unwrap().interaction_id;
    let picker="Escolha B\n❯ 1. primeira\n  2. segunda\n  3. Type something.\n  4. Chat about this\nEnter to select · ↑/↓ to navigate · Esc to cancel\n".to_string();
    let closed="────────────────────────────────\n❯ \n────────────────────────────────\n".to_string();
    *f.io.panes.lock().unwrap()=vec![picker.clone(),picker.clone(),picker.clone(),picker,closed];
    // Corpo torto: recusa com o código e o executor segue de pé para a resposta seguinte.
    let bad=h.control("ask-bad".into(),"answer_questions".into(),json!({"request_id":"ask:B","interaction_id":interaction,"answers":[{"kind":"option","indices":"x"}]})).await;
    assert_eq!(bad.unwrap_err().code,"terminal_payload");
    let reply=h.control("ask-keys".into(),"answer_questions".into(),json!({"request_id":"ask:B","interaction_id":interaction,"answers":[{"kind":"option","indices":[0],"labels":["primeira"]}]})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["code"].clone()),(hangar_server::runtime::protocol::Disposition::Accepted,json!("answered")),"{}",reply.payload);
    let keys:Vec<String>=f.io.calls.lock().unwrap().iter().filter(|c|c.args[0]=="send-keys").map(|c|c.args.last().unwrap().clone()).collect();
    assert_eq!(keys,["\r"]);
    assert!(tokio::time::timeout(Duration::ZERO,poll.wait()).await.is_err(),"nada entregue ao hook sem receipt_v2");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_common_and_two_held_surfaces_use_local_services() {
    use hangar_server::plugin::{Ask,PermissionGate,Receipt};
    use hangar_server::terminal_facts::LocalTerminalFacts;
    for form_first in [false,true] {
        let f=Fixture::new().await;let h=f.start();
        assert_eq!(h.command(f.command("common","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
        f.owner(true);f.io.calls.lock().unwrap().clear();let binding=LocalTerminalFacts::binding(&f.target);
        let gate=PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false};
        let approval=f.plugin.begin_ask(&binding,Ask {id:"perm:A".into(),questions:None,tool:Some("Bash".into()),janela_ms:None,summary:Some("comando A".into())},gate,std::time::Instant::now()).unwrap();
        let form=f.plugin.begin_ask(&binding,Ask {id:"ask:B".into(),questions:Some(vec![json!({"question":"Escolha B","options":[{"label":"primeira"}]})]),tool:None,janela_ms:None,summary:None},gate,std::time::Instant::now()).unwrap();
        let initial=f.plugin.pending_all(&binding,std::time::Instant::now()).unwrap();assert_eq!(initial.len(),2);
        let a=initial.iter().find(|question|question.id=="perm:A").unwrap().interaction_id.clone();let b=initial.iter().find(|question|question.id=="ask:B").unwrap().interaction_id.clone();
        let expected=f.plugin.interactions().expected(&binding,Some(&a)).unwrap();let local=LocalTerminalFacts::new(f.io.clone(),Some(f.plugin.clone()),None);
        assert_eq!(local.plugin_question(&f.target,&expected).await.unwrap().id,"perm:A");
        let mut wrong=expected.clone();wrong.interaction_id=b.clone();assert_eq!(local.plugin_question(&f.target,&wrong).await.unwrap_err().0,"erro_interacao_desatualizada");
        let mut wrong=expected.clone();wrong.semantic["resumo"]=json!("mudou");assert!(local.plugin_question(&f.target,&wrong).await.is_err());
        let mut wrong=expected;wrong.source_revision=Some("outra revisão".into());assert!(local.plugin_question(&f.target,&wrong).await.is_err());
        let mut cases=vec![("perm:A",approval,"select",json!({"request_id":"perm:A","interaction_id":a,"option":1})),("ask:B",form,"answer_questions",json!({"request_id":"ask:B","interaction_id":b,"answers":[{"kind":"option","indices":[0],"labels":["primeira"]}]}))];
        if form_first {cases.reverse();}
        for (index,(request,poll,control,payload)) in cases.into_iter().enumerate() {
            let hc=h.clone();let operation=format!("response:{request}");let op=operation.clone();let control=control.to_owned();let expected_id=payload["interaction_id"].clone();
            let command=tokio::spawn(async move {hc.control(op,control,payload).await});let body=tokio::time::timeout(WAIT,poll.wait()).await.unwrap();
            if request=="perm:A" {assert_eq!(body["permitir"],true);assert!(body.get("answers").is_none());}else {assert_eq!(body["answers"],json!({"Escolha B":"primeira"}));}
            assert_eq!(f.state()["operations"][&operation]["wire_attempts"][format!("terminal:1:{operation}")]["status"],"dispatching");
            let receipt:Receipt=serde_json::from_value(json!({"publication_id":body["publication_id"],"generation":body["generation"],"session_id":body["session_id"]})).unwrap();f.plugin.finish(&binding,request,"app",Some(receipt)).unwrap();
            let reply=command.await.unwrap().unwrap();assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted);assert_eq!(reply.payload["interaction_id"],expected_id);
            let remaining=f.plugin.pending_all(&binding,std::time::Instant::now()).unwrap();if index==0 {assert_eq!(remaining,vec![initial.iter().find(|question|question.id!=request).unwrap().clone()]);}else {assert!(remaining.is_empty());}
        }
        assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));h.stop().await.unwrap();
    }
}
#[cfg(target_os="linux")]
#[tokio::test]
async fn terminal_runtime_same_process_new_conversation_without_native_record_never_writes_old_binding() {
    let f=Fixture::new().await;let h=f.start();
    let native=f.target.metadata.config_dir.join("sessions").join(format!("{}.json",f.target.metadata.agent_pid));
    *f.io.native_record.lock().unwrap()=None;std::fs::remove_file(native).unwrap();
    let current=f.target.transcript.parent().unwrap().join("sid-B.jsonl");std::fs::write(&current,"").unwrap();let _current_fd=std::fs::File::open(&current).unwrap();
    let local=hangar_server::terminal_facts::LocalTerminalFacts::new(f.io.clone(),Some(f.plugin.clone()),None);
    assert_eq!(local.read(&f.target,&f.target.binding,"").await.unwrap_err().0,"terminal_conversation_changed");
    f.owner(true);let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    let poll=f.plugin.begin_ask(&binding,hangar_server::plugin::Ask {id:"perm:old-sid".into(),questions:None,tool:Some("Bash".into()),janela_ms:None,summary:None},hangar_server::plugin::PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false},std::time::Instant::now()).unwrap();
    let interaction=f.plugin.pending(&binding,std::time::Instant::now()).unwrap().interaction_id;
    assert_eq!(h.control("old-answer".into(),"select".into(),json!({"option":1,"request_id":"perm:old-sid","interaction_id":interaction})).await.unwrap_err().code,"terminal_conversation_changed");
    assert!(tokio::time::timeout(Duration::ZERO,poll.wait()).await.is_err(),"Store::answer não entrega resposta em conversa sem prova");
    let reply=h.command(f.command("old-sid","Não pode chegar à conversa B")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Deferred);assert_eq!(reply.payload["code"],"terminal_facts");
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]=="display-message"));
    assert!(f.io.socket_calls.lock().unwrap().is_empty());assert!(f.plugin_requests.lock().unwrap().is_empty());
    assert!(f.state()["rows"].as_array().unwrap().iter().all(|row|row["delivered"]!=true));h.stop().await.unwrap();
}
/// Sem o registro nativo, só o `/pull` aberto do plugin prova a conversa, e o `Fill` entrega esse
/// `/pull` ao plugin: a prova sumia até ele voltar, o texto ficava no composer e a fila travava. A
/// entrega vai pelo teclado, que deixa o `/pull` aberto.
#[cfg(target_os="linux")]
#[tokio::test]
async fn terminal_runtime_fill_never_spends_the_only_conversation_proof() {
    let f=Fixture::new().await;
    let native=f.target.metadata.config_dir.join("sessions").join(format!("{}.json",f.target.metadata.agent_pid));
    *f.io.native_record.lock().unwrap()=None;std::fs::remove_file(native).unwrap();
    f.fill_plugin.store(true,std::sync::atomic::Ordering::Release);f.repull_ms.store(500,std::sync::atomic::Ordering::Release);
    let h=f.start();let binding=hangar_server::terminal_facts::LocalTerminalFacts::binding(&f.target);
    f.wait_for("o /pull do plugin",||f.plugin.waiting(&binding).unwrap_or(false)).await;
    let reply=h.command(f.command("fill","hello @peer")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{}",reply.payload);
    assert!(f.io.text.lock().unwrap().is_empty(),"nada fica digitado no composer");
    h.stop().await.unwrap();
}
/// Registro nativo ausente para o pid do agente e plugin sem `/pull`: nada prova a conversa. A sessão
/// aparece como sem entrega segura antes de uma mensagem travar, e o aviso sai quando a prova volta.
#[tokio::test]
async fn terminal_runtime_missing_native_record_and_silent_plugin_mark_no_safe_delivery() {
    let f=Fixture::new().await;
    let native=f.target.metadata.config_dir.join("sessions").join(format!("{}.json",f.target.metadata.agent_pid));
    let saved=f.io.native_record.lock().unwrap().take();std::fs::remove_file(native).unwrap();
    let h=f.start_full(broadcast::channel(128).0,Duration::from_millis(50));
    f.wait_for("sem entrega segura",||f.status.borrow().input_stalled.as_deref()==Some("no_safe_delivery")).await;
    assert_eq!(h.snapshot().await.unwrap()["view"]["input_stalled"],"no_safe_delivery");
    *f.io.native_record.lock().unwrap()=saved;f.io.refresh_record();
    f.wait_for("o aviso sai com a prova de volta",||f.status.borrow().input_stalled.is_none()).await;
    h.stop().await.unwrap();
}
/// Vida nova: o Claude ainda subindo não tem registro nativo nem conversa aberta. A primeira mensagem espera
/// na fila sem erro na sessão e sai quando a prova chega; passado o prazo de subida, a falta de prova é erro.
#[tokio::test]
async fn terminal_runtime_a_starting_life_without_its_conversation_waits_without_an_error() {
    let f=Fixture::new().await;
    let native=f.target.metadata.config_dir.join("sessions").join(format!("{}.json",f.target.metadata.agent_pid));
    let saved=f.io.native_record.lock().unwrap().take();std::fs::remove_file(&native).unwrap();
    // O agente do teste é este processo: o prazo cobre a idade dele numa suíte longa.
    let h=f.start_grace(Duration::from_secs(24*3600));
    assert_eq!(h.command(f.command("primeira","oi")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert!(h.drain().await.is_ok(),"subindo não é erro");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(h.snapshot().await.unwrap()["error"].is_null(),"nenhum tique publicou erro");
    assert!(f.status.borrow().error.is_none(),"a sessão não mostra problema");
    *f.io.native_record.lock().unwrap()=saved;f.io.refresh_record();
    f.wait_for("a primeira mensagem sai com a prova",||!typed_at(&f,"oi").is_empty()).await;
    h.stop().await.unwrap();

    let f=Fixture::new().await;
    let native=f.target.metadata.config_dir.join("sessions").join(format!("{}.json",f.target.metadata.agent_pid));
    f.io.native_record.lock().unwrap().take();std::fs::remove_file(&native).unwrap();
    let h=f.start_grace(Duration::ZERO);
    h.command(f.command("primeira","oi")).await.unwrap();
    assert_eq!(h.drain().await.unwrap_err().code,"terminal_facts","passado o prazo de subida");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_unreadable_capture_recovers_without_sse() {
    let f=Fixture::new().await;f.ready.store(false,std::sync::atomic::Ordering::Release);let h=f.start();
    assert_eq!(h.command(f.command("waiting-capture","Quando voltar")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    f.io.capture_error.store(true,std::sync::atomic::Ordering::Release);f.ready.store(true,std::sync::atomic::Ordering::Release);
    assert_eq!(h.drain().await.unwrap_err().code,"terminal_facts");assert!(typed_at(&f,"Quando voltar").is_empty());
    f.io.capture_error.store(false,std::sync::atomic::Ordering::Release);f.wait_for("a mesma fila retoma",||!typed_at(&f,"Quando voltar").is_empty()).await;
    assert!(h.snapshot().await.unwrap()["error"].is_null());h.stop().await.unwrap();assert_eq!(typed_at(&f,"Quando voltar").len(),1);
}
// Binding Windows simulado: a sessão de logon não se lê fora do Windows, e a área de transferência
// fica indisponível. No Windows o pane do teste é este processo, na sessão do servidor, e escreveria
// na área de transferência real; ali a regra da sessão é coberta em `terminal_facts`.
#[cfg(not(windows))]
#[tokio::test]
async fn terminal_runtime_windows_clipboard_unavailable_does_not_write_multiline() {
    let mut f=Fixture::new().await;f.target.binding.windows=true;f.target.binding.pane="=session:0.0".into();f.target.binding.clipboard_lock_path=Some(f._dir.path().join("clipboard.lock"));
    let h=f.start();let reply=h.command(f.command("clipboard","linha1\nlinha2")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Deferred);assert_eq!(reply.payload["code"],"clipboard_unavailable");
    assert!(f.io.calls.lock().unwrap().iter().all(|call|call.args[0]!="send-keys"));h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_binding_changes_while_an_operation_waits_prevents_old_second_effect() {
    let f=Fixture::new().await; f.io.blocked.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let hc=h.clone(); let cmd=f.command("flight","Primeiro"); let first=tokio::spawn(async move {hc.command(cmd).await});
    f.wait_for("espera 4",||f.io.calls.lock().unwrap().iter().any(|r|r.args[0]=="send-keys")).await;
    let hc=h.clone(); let cmd=f.command("waiting","Segundo"); let second=tokio::spawn(async move {hc.command(cmd).await});
    f.generation.store(2,std::sync::atomic::Ordering::Release); f.io.blocked.store(false,std::sync::atomic::Ordering::Release); f.io.gate.notify_waiters();
    assert_eq!(first.await.unwrap().unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    assert_eq!(second.await.unwrap().unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys").count(),1); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_deferred_without_submission_never_repeats_same_operation() {
    let f=Fixture::new().await; f.io.fail_write.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    // O texto parcial pode ser limpo antes de Enter; o comando ainda não alterou a conversa.
    let result=h.command(f.command("clear","/clear")).await.unwrap();
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    let count=f.io.calls.lock().unwrap().len(); h.command(f.command("clear","/clear")).await.unwrap(); assert_eq!(f.io.calls.lock().unwrap().len(),count);
    assert!(f.state()["rows"].as_array().unwrap().is_empty()); assert_ne!(f.state()["runtime_state"]["preserve_binding"],true);
    f.io.fail_write.store(false,std::sync::atomic::Ordering::Release);
    assert_eq!(h.command(f.command("after-deferred-clear","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_native_receipt_for_queue_attempt_keeps_root_uuid_and_does_not_confirm() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    let command=f.command("native-root","[de: peer] Olá"); h.command(command.clone()).await.unwrap();
    f.ready.store(true,std::sync::atomic::Ordering::Release); f.native.store(true,std::sync::atomic::Ordering::Release); h.drain().await.unwrap();
    assert_eq!(f.io.socket_calls.lock().unwrap().len(),1);
    let envelope:Value=serde_json::from_slice(&f.io.socket_calls.lock().unwrap()[0]).unwrap(); assert_eq!(envelope["msg_id"].as_str().unwrap().len(),36);assert_eq!(envelope["from"],"uds:fixture-inbox");
    h.queue("receipt".into(),Action::Finish {id:"native-root".into(),status:queue::Status::Accepted,result:json!({"operation_id":"native-root","disposition":"accepted","payload":{"native_status":"delivered"}})}).await.unwrap();
    let replay=h.command(command).await.unwrap(); assert_eq!(replay.payload["native"],true); assert_eq!(replay.payload["native_status"],"delivered"); assert_ne!(f.state()["rows"][0]["confirmed"],true);
    assert_eq!(f.io.socket_calls.lock().unwrap().len(),1); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_native_refusal_is_visible_and_unrelated_finish_is_rejected() {
    let f=Fixture::new().await; f.native.store(true,std::sync::atomic::Ordering::Release); let h=f.start(); h.command(f.command("native","[de: peer] Olá")).await.unwrap();
    h.queue("refused".into(),Action::Finish {id:"native".into(),status:queue::Status::Rejected,result:json!({"operation_id":"native","disposition":"rejected","payload":{"native_status":"refused"}})}).await.unwrap();
    assert_eq!(f.state()["rows"][0]["desistiu"],true); h.drain().await.unwrap(); assert_eq!(f.io.socket_calls.lock().unwrap().len(),1);
    h.command(f.command("ordinary","Olá")).await.unwrap();
    assert!(h.queue("forge".into(),Action::Finish {id:"ordinary".into(),status:queue::Status::Accepted,result:json!({"operation_id":"ordinary","disposition":"accepted","payload":{"native_status":"delivered"}})}).await.is_err()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_after_dispatch_holds_old_binding_until_detach() {
    let f=Fixture::new().await; f.io.fail_enter.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let result=h.command(f.command("clear-now","/clear")).await.unwrap(); assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    assert_eq!(f.state()["runtime_state"]["preserve_binding"],true);
    let count=f.io.calls.lock().unwrap().len(); h.command(f.command("clear-now","/clear")).await.unwrap(); assert_eq!(f.io.calls.lock().unwrap().len(),count);
    assert!(h.command(f.command("old-life","Olá")).await.is_err());
    assert_eq!(f.io.calls.lock().unwrap().len(),count);
    // A tecla do espelho não escreve na conversa: segue com a trava, como no Python.
    assert_eq!(h.control("old-key".into(),"interactive_key".into(),json!({"key":"C-c"})).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let count=f.io.calls.lock().unwrap().len();
    h.drain().await.unwrap(); assert_eq!(f.io.calls.lock().unwrap().len(),count); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_restart_after_clear_dispatch_conserves_barrier() {
    let f=Fixture::new().await;
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64};
    store.exec(1,"prepare",clock,Action::Prepare {id:"clear-crash".into(),payload:serde_json::to_value(f.command("clear-crash","/clear")).unwrap(),entry_id:None}).unwrap();
    store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"clear-crash".into(),wire_id:"terminal:1:clear-crash".into(),staged:false}).unwrap(); drop(store);
    let h=f.start();
    assert!(h.command(f.command("after-crash","Olá")).await.is_err()); assert_eq!(f.state()["runtime_state"]["preserve_binding"],true);
    assert!(f.io.calls.lock().unwrap().is_empty()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_unproved_before_enter_raises_no_barrier() {
    // #84: o texto não chegou ao composer (foco no painel de agentes), o Enter nunca saiu.
    let f=Fixture::new().await; f.io.swallow.store(true,std::sync::atomic::Ordering::Release); let h=f.start_clear(Duration::from_secs(30));
    let result=h.command(f.command("clear-lost","/clear")).await.unwrap();
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Unknown,"{}",result.payload); assert_eq!(result.payload["stage"],"input_proof");
    assert!(f.state()["runtime_state"]["clear_barrier"].is_null());
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="\r"),"o Enter do /clear não pode ter saído");
    f.io.swallow.store(false,std::sync::atomic::Ordering::Release);
    assert_eq!(h.command(f.command("after-lost-clear","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    h.stop().await.unwrap();
    // A reabertura usa o mesmo critério.
    let h=f.start_clear(Duration::from_secs(30)); assert!(f.state()["runtime_state"]["clear_barrier"].is_null());
    assert_eq!(h.command(f.command("after-restart","Olá de novo")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_without_new_conversation_releases_barrier_without_resending() {
    // O Enter saiu e o composer esvaziou, mas a conversa nunca mudou: o /clear caiu em outro lugar.
    let f=Fixture::new().await; let h=f.start_clear(Duration::from_millis(200));
    // Transcript de antes do despacho (outra sessão, ou conversa antiga nascida de /clear): não prova nada.
    std::fs::write(f.target.transcript.with_file_name("00000000-0000-4000-8000-000000000001.jsonl"),"{\"message\":{\"content\":\"<command-name>/clear</command-name>\"}}\n").unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(h.command(f.command("clear-stuck","/clear")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    // A trava só vive 200 ms aqui: conferir a recusa no meio dela dependia da velocidade da máquina. O desfecho
    // `clear_not_applied` abaixo prova que ela subiu; a recusa durante a trava é a do teste seguinte.
    h.queue("producer".into(),Action::Append {text:"Na fila".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("queued-row".into())}).await.unwrap();
    f.wait_for("trava sai",||f.state()["runtime_state"]["clear_barrier"].is_null()).await;
    // A saída grava a trava e depois a recusa: o retrato passa pelo ator e só volta com as duas no diário.
    let view=h.snapshot().await.unwrap();
    let op=&f.state()["operations"]["clear-stuck"]; assert_eq!(op["status"],"rejected"); assert_eq!(op["result"]["payload"]["code"],"clear_not_applied");
    assert_ne!(f.state()["runtime_state"]["preserve_binding"],true);
    assert_eq!(view["view"]["input_stalled"],"clear_not_applied");
    f.wait_for("fila segue",||f.state()["rows"].as_array().unwrap().iter().any(|r|r["id"]=="queued-row" && r["delivered"]==true)).await;
    assert_eq!(h.command(f.command("after-release","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let typed=|t:&str|f.io.calls.lock().unwrap().iter().filter(|r|r.args.contains(&"-l".into()) && r.args.last().unwrap()==t).count();
    assert_eq!(typed("/clear"),1,"o /clear nunca é reenviado sozinho");
    h.stop().await.unwrap();
    // O /clear recusado pela saída não ergue a trava na reabertura.
    let h=f.start_clear(Duration::from_secs(30)); assert!(f.state()["runtime_state"]["clear_barrier"].is_null());
    assert_eq!(h.command(f.command("after-reopen","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_with_new_transcript_on_disk_keeps_barrier() {
    // O transcript novo do /clear existe: falta só o Python trocar o vínculo, a trava fica.
    let f=Fixture::new().await; let h=f.start_clear(Duration::from_millis(100));
    assert_eq!(h.command(f.command("clear-ok","/clear")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    std::fs::write(f.target.transcript.with_file_name("00000000-0000-4000-8000-000000000002.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/clear</command-name>\"}}\n").unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(f.state()["runtime_state"]["clear_barrier"].is_object()); assert_ne!(f.state()["operations"]["clear-ok"]["status"],"rejected");
    assert!(h.command(f.command("old-life","Olá")).await.is_err());
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_while_busy_waits_for_the_turn_before_releasing() {
    // Com o Claude trabalhando o /clear espera na fila do próprio Claude Code e roda no fim do turno.
    let f=Fixture::new().await; let h=f.start_clear(Duration::from_millis(100));
    assert_eq!(h.command(f.command("clear-busy","/clear")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(f.state()["runtime_state"]["clear_barrier"].is_object());
    f.idle.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("trava sai com a sessão parada",||f.state()["runtime_state"]["clear_barrier"].is_null()).await;
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_barrier_lets_controls_through_while_the_turn_runs() {
    // A trava espera o turno, mas Parar e as teclas do espelho não escrevem na conversa nova.
    let f=Fixture::new().await; let h=f.start_clear(Duration::from_millis(100));
    assert_eq!(h.command(f.command("clear-busy","/clear")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    assert!(f.state()["runtime_state"]["clear_barrier"].is_object());
    let stop=h.control("stop-during-clear".into(),"interrupt".into(),json!({"clear":false})).await;
    assert_eq!(stop.as_ref().map(|r|r.disposition).map_err(|e|e.code.clone()),Ok(hangar_server::runtime::protocol::Disposition::Accepted));
    let key=h.control("key-during-clear".into(),"navigation_key".into(),json!({"key":"Down"})).await;
    assert_eq!(key.as_ref().map(|r|r.disposition).map_err(|e|e.code.clone()),Ok(hangar_server::runtime::protocol::Disposition::Accepted));
    let keys:Vec<String>=f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys").map(|r|r.args.last().unwrap().clone()).collect();
    assert!(keys.iter().any(|k|k=="Escape") && keys.iter().any(|k|k=="Down"),"{keys:?}");
    assert!(f.state()["runtime_state"]["clear_barrier"].is_object(),"o controle não solta a trava");
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_unreadable_composer_holding_the_queue_shows_why() {
    // O desenho do composer mudou: nada é digitado às cegas, mas a vista diz por que a fila não anda.
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release);
    let h=f.start_full(broadcast::channel(128).0,Duration::from_millis(50));
    h.queue("append-u".into(),Action::Append {text:"Parada".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("u".into())}).await.unwrap();
    let start=std::time::Instant::now();
    while h.snapshot().await.unwrap()["view"]["input_stalled"]!="composer_unreadable" {
        assert!(start.elapsed()<WAIT,"o motivo nunca apareceu: {}",h.snapshot().await.unwrap()["view"]); tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Parada"));
    f.ready.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("entrega com o composer legível",||f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Parada")).await;
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_accepted_without_proof_is_given_up_and_rescued_when_it_shows_up() {
    // Eco que não casa ou prompt barrado por hook: com o Claude parado, a bolha vira "não chegou" (o
    // Descartar a tira), sem reenviar; a mensagem que aparece depois no transcript volta a ser entregue.
    let f=Fixture::new().await; let h=f.start_give_up(Duration::from_millis(200));
    assert_eq!(h.command(f.command("lost","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    f.wait_for("desistida no prazo",||f.state()["rows"][0]["desistiu"]==true).await;
    let typed=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.contains(&"-l".into()) && r.args.last().unwrap()=="Olá").count();
    assert_eq!(typed(),1,"nunca reenviada");
    std::fs::write(&f.target.transcript,format!("{}\n",json!({"type":"user","sessionId":"sid","uuid":"late","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"Olá"}}))).unwrap();
    h.confirm().await.unwrap();
    assert_eq!(f.state()["rows"][0]["confirmed"],true); assert!(f.state()["rows"][0]["desistiu"].is_null());
    h.stop().await.unwrap();
    // A incerta desistida (`GiveUp`) também é resgatada.
    let f=Fixture::new().await; f.io.fail_enter.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    assert_eq!(h.command(f.command("unsure","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Unknown);
    assert_eq!(h.queue("give-up".into(),Action::GiveUp {entry_id:"unsure".into()}).await.unwrap(),json!(true));
    assert_eq!(f.state()["rows"][0]["desistiu"],true);
    std::fs::write(&f.target.transcript,format!("{}\n",json!({"type":"user","sessionId":"sid","uuid":"late","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"Olá"}}))).unwrap();
    h.confirm().await.unwrap();
    assert_eq!(f.state()["rows"][0]["confirmed"],true,"{}",f.state()["rows"]);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_question_scrolled_off_screen_holds_the_queue() {
    // O menu do AskUserQuestion saiu da tela: as teclas do recado navegariam o picker. A fila espera a resposta.
    let f=Fixture::new().await;
    let config=f.target.metadata.config_dir.clone();std::fs::create_dir_all(config.join(".hangar-askq")).unwrap();
    std::fs::write(&f.target.transcript,"{\"timestamp\":\"2099-01-01T00:00:00Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_q\",\"name\":\"AskUserQuestion\"}]}}\n").unwrap();
    std::fs::write(config.join(".hangar-askq/sid.json"),json!({"transcript_path":f.target.transcript,"tool_use_id":"toolu_q","tool_input":{"questions":[
        {"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}).to_string()).unwrap();
    let h=f.start();
    h.queue("append-q".into(),Action::Append {text:"Recado".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("q".into())}).await.unwrap();
    assert_eq!(h.drain().await.unwrap()["drained"],0);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Recado"),"nada digitado com a pergunta aberta");
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().append(true).open(&f.target.transcript).unwrap(),"{{\"timestamp\":\"2099-01-01T00:00:01Z\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_q\"}}]}}}}").unwrap();
    f.wait_for("entrega depois da resposta",||f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Recado")).await;
    h.stop().await.unwrap();
    // Sidecar de uma vida que morreu na pergunta (mais velho que o processo): a conversa retomada não a mostra
    // nem dispara o PostToolUse, e a fila não espera por ela.
    let f=Fixture::new().await;
    let config=f.target.metadata.config_dir.clone();std::fs::create_dir_all(config.join(".hangar-askq")).unwrap();
    std::fs::write(&f.target.transcript,"{\"timestamp\":\"2099-01-01T00:00:00Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_q\",\"name\":\"AskUserQuestion\"}]}}\n").unwrap();
    let sidecar=config.join(".hangar-askq/sid.json");
    std::fs::write(&sidecar,json!({"transcript_path":f.target.transcript,"tool_use_id":"toolu_q","tool_input":{"questions":[
        {"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}).to_string()).unwrap();
    std::fs::OpenOptions::new().write(true).open(&sidecar).unwrap().set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH+Duration::from_secs(1))).unwrap();
    let h=f.start();
    h.queue("append-old".into(),Action::Append {text:"Recado".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("old".into())}).await.unwrap();
    f.wait_for("entrega sem esperar a pergunta morta",||f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Recado")).await;
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_footer_focus_escape_has_a_ceiling_per_row() {
    // O Esc devolve o foco do painel de agentes; se ele não volta, a linha tenta duas vezes e espera.
    let f=Fixture::new().await; f.io.footer.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let result=h.command(f.command("stuck-focus","Olá")).await.unwrap();
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Deferred); assert_eq!(result.payload["code"],"footer_focus");
    let escapes=||f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys" && r.args.last().unwrap()=="Escape").count();
    f.wait_for("segunda tentativa",||escapes()==2).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(escapes(),2,"o Esc do rodapé não vira laço de teclas");
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Olá"));
    f.io.footer.store(false,std::sync::atomic::Ordering::Release);
    f.wait_for("entrega depois do foco voltar",||f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="Olá")).await;
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_restart_with_stale_clear_barrier_still_expires() {
    // A trava gravada de uma vida anterior, sem conversa nova, também tem saída.
    let f=Fixture::new().await;
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64};
    store.exec(1,"prepare",clock,Action::Prepare {id:"clear-crash".into(),payload:serde_json::to_value(f.command("clear-crash","/clear")).unwrap(),entry_id:None}).unwrap();
    store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"clear-crash".into(),wire_id:"terminal:1:clear-crash".into(),staged:false}).unwrap(); drop(store);
    let h=f.start_clear(Duration::from_millis(100));
    assert!(h.command(f.command("after-crash","Olá")).await.is_err());
    f.wait_for("trava sai",||f.state()["runtime_state"]["clear_barrier"].is_null()).await;
    assert_eq!(h.command(f.command("after-expire","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    assert!(!f.io.calls.lock().unwrap().iter().any(|r|r.args.last().unwrap()=="/clear")); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_claim_crash_before_intent_restores_safe_pending_row() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release);
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64};
    store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("stray".into())}).unwrap();
    store.exec(1,"terminal:queue:999",clock,Action::Claim {min_ts:1.0,limit:Some(1),entry_id:None}).unwrap(); drop(store);
    let h=f.start(); h.snapshot().await.unwrap(); assert_eq!(f.state()["rows"][0]["delivered"],false);
    f.ready.store(true,std::sync::atomic::Ordering::Release); h.drain().await.unwrap(); assert_eq!(f.state()["rows"][0]["delivered"],true); assert_eq!(f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys" && r.args.last().unwrap()=="Olá").count(),1); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_claims_one_batch_while_driver_is_in_flight() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    h.command(f.command("first-claim","Um")).await.unwrap(); h.command(f.command("second-claim","Dois")).await.unwrap();
    f.io.blocked.store(true,std::sync::atomic::Ordering::Release); f.ready.store(true,std::sync::atomic::Ordering::Release);
    let hc=h.clone(); let draining=tokio::spawn(async move {hc.drain().await});
    f.wait_for("espera 5",||f.io.calls.lock().unwrap().iter().any(|r|r.args[0]=="send-keys")).await;
    let rows=f.state()["rows"].clone();assert_eq!(rows.as_array().unwrap().len(),1);
    assert_eq!(rows[0]["delivered"],true);assert_eq!(rows[0]["text"],"Um\nDois");
    let members:Vec<_>=rows[0]["batch_entries"].as_array().unwrap().iter().map(|row|row["id"].as_str().unwrap()).collect();
    assert_eq!(members,["first-claim","second-claim"]);
    f.io.blocked.store(false,std::sync::atomic::Ordering::Release); f.io.gate.notify_waiters(); draining.await.unwrap().unwrap(); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_prepared_without_dispatch_is_deferred_not_unknown() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release);
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64}; let command=f.command("prepared","Olá");
    store.exec(1,"prepare",clock,Action::Prepare {id:"prepared".into(),payload:serde_json::to_value(&command).unwrap(),entry_id:Some("prepared".into())}).unwrap();
    store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("prepared".into())}).unwrap(); drop(store);
    let h=f.start(); assert_eq!(h.command(command).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(f.state()["rows"][0]["delivered"],false); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_invalid_prompt_is_rejected_before_queue_and_dispatch() {
    let f=Fixture::new().await; let h=f.start();
    for (id,text) in [("empty",""),("blank"," \n\t"),("escape","bad\u{1b}"),("delete","bad\u{7f}")] {
        assert!(h.command(f.command(id,text)).await.is_err()); assert!(f.state()["operations"].get(id).is_none());
    }
    assert!(f.state()["rows"].as_array().unwrap().is_empty()); assert!(f.io.calls.lock().unwrap().is_empty()); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_interactive_text_keeps_whitespace_character() {
    let f=Fixture::new().await; let h=f.start();
    let result=h.control("space".into(),"terminal_input".into(),json!({"text":" "})).await.unwrap();
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    assert!(f.io.calls.lock().unwrap().iter().any(|r|r.args[0]=="send-keys" && r.args.last().unwrap()==" ")); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_clear_new_conversation_after_enter_is_accepted_and_old_life_stays_blocked() {
    let f=Fixture::new().await; f.io.rotate_enter.store(true,std::sync::atomic::Ordering::Release); let h=f.start();
    let result=h.command(f.command("clear-changes-sid","/clear")).await.unwrap();
    // A troca de conversa é o efeito do próprio /clear: o composer vazio no mesmo pane prova a submissão.
    assert_eq!(result.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{}",result.payload); assert_eq!(result.payload["code"],"submitted");
    assert_eq!(*f.io.conversation.lock().unwrap(),"new-sid"); let snapshot=h.snapshot().await.unwrap(); assert_eq!(snapshot["view"]["conversation"],"sid"); assert_eq!(snapshot["view"]["preserve_binding"],true);
    assert!(h.command(f.command("after-sid-change","Olá")).await.is_err()); h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_prepare_without_append_restores_prompt_and_delivers_once() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release);
    let text="Olá 🌎 C:\\text — 📎 imagem: /tmp/x.png";
    let mut command=f.command("prepared-missing",text); command.payload["pre_transcript"]=json!(true);
    let mut original=serde_json::to_value(&command).unwrap(); original["payload"]["_terminal_generation"]=json!(1);
    let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"terminal:queue:1",ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64},Action::Prepare {id:command.operation_id.clone(),payload:original,entry_id:Some(command.operation_id.clone())}).unwrap(); drop(store);
    let h=f.start(); h.snapshot().await.unwrap();
    let rows=f.state()["rows"].clone(); assert_eq!(rows.as_array().unwrap().len(),1); assert_eq!(rows[0]["id"],"prepared-missing"); assert_eq!(rows[0]["text"],text); assert_eq!(rows[0]["pre_transcript"],true);
    let replay=h.command(command.clone()).await.unwrap(); assert_eq!(replay.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    f.ready.store(true,std::sync::atomic::Ordering::Release); h.drain().await.unwrap(); h.command(command).await.unwrap(); h.drain().await.unwrap();
    assert_eq!(f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys" && r.args.last().unwrap()==text).count(),1); assert_eq!(f.state()["rows"].as_array().unwrap().len(),1); h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_native_receipt_matches_queue_append_without_submit_root() {
    for (native_status,status) in [("delivered",queue::Status::Accepted),("refused",queue::Status::Rejected)] {
        let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
        h.queue("producer".into(),Action::Append {text:"[de: peer] Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("producer-entry".into())}).await.unwrap();
        f.ready.store(true,std::sync::atomic::Ordering::Release); f.native.store(true,std::sync::atomic::Ordering::Release); h.drain().await.unwrap();
        assert!(f.state()["operations"].get("producer-entry").is_none());
        h.queue("native-ack".into(),Action::Finish {id:"producer-entry".into(),status,result:json!({"operation_id":"producer-entry","disposition":if native_status=="delivered"{"accepted"}else{"rejected"},"payload":{"native_status":native_status}})}).await.unwrap();
        let row=f.state()["rows"][0].clone(); assert_ne!(row["confirmed"],true); if native_status=="refused" {assert_eq!(row["desistiu"],true);}
        h.drain().await.unwrap(); assert_eq!(f.io.socket_calls.lock().unwrap().len(),1); assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys")); h.stop().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_runtime_windows_line_breaks_are_typed_as_plain_newlines() {
    let f=Fixture::new().await; let h=f.start();
    let reply=h.command(f.command("pasted","Pessoal:\r\nlinha dois\rtrês")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{}",reply.payload);
    assert_eq!(f.state()["rows"][0]["text"],"Pessoal:\nlinha dois\ntrês");
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args.iter().all(|a|!a.contains('\r') || a=="\r")),"nenhum \\r dentro do texto digitado");
    h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_input_primitive_payload_is_rejected_without_stopping_actor() {
    let f=Fixture::new().await; let h=f.start();
    for payload in [Value::Null,json!(false),json!([]),json!("")] {
        assert!(h.control("malformed".into(),"input".into(),payload).await.is_err()); h.snapshot().await.unwrap();
    }
    assert!(f.state()["rows"].as_array().unwrap().is_empty());assert!(f.io.calls.lock().unwrap().is_empty());h.stop().await.unwrap();
}

#[tokio::test]
async fn terminal_runtime_removed_recovered_root_has_no_further_delivery_or_publication() {
    for exhausted in [false,true] {
        let f=Fixture::new().await;let clock=ClockSample {monotonic_s:0.0,epoch_s:chrono::Utc::now().timestamp() as f64};
        let mut original=serde_json::to_value(f.command("removed-root","[de: peer] Olá")).unwrap();original["payload"]["_terminal_generation"]=json!(1);
        let mut store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
        store.exec(1,"prepare-root",clock,Action::Prepare {id:"removed-root".into(),entry_id:Some("removed-root".into()),payload:original}).unwrap();
        store.exec(1,"recover-create",clock,Action::Recover).unwrap();
        if exhausted {for n in 0..2 {store.exec(1,&format!("bump:{n}"),clock,Action::BumpAttempts {entry_id:"removed-root".into()}).unwrap();}}
        store.exec(1,"prepare-attempt",clock,Action::Prepare {id:"old-attempt".into(),entry_id:Some("removed-root".into()),payload:json!({"operation_id":"old-attempt","kind":"input","payload":{"text":"[de: peer] Olá","_terminal_generation":1}})}).unwrap();
        store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"old-attempt".into(),wire_id:"terminal:1:old-attempt".into(),staged:false}).unwrap();
        store.exec(1,"finish",clock,Action::Finish {id:"old-attempt".into(),status:if exhausted{queue::Status::Deferred}else{queue::Status::Rejected},result:json!({"operation_id":"old-attempt","disposition":if exhausted{"deferred"}else{"rejected"},"payload":{"cleanup":"proved"}})}).unwrap();
        store.exec(1,"remove",clock,Action::Remove {entry_id:"removed-root".into()}).unwrap();drop(store);
        f.native.store(true,std::sync::atomic::Ordering::Release);f.unknown.store(true,std::sync::atomic::Ordering::Release);
        let h=f.start();h.snapshot().await.unwrap();h.drain().await.unwrap();h.drain().await.unwrap();
        assert!(f.state()["rows"].as_array().unwrap().is_empty());assert!(f.io.socket_calls.lock().unwrap().is_empty());assert!(f.io.calls.lock().unwrap().is_empty());h.stop().await.unwrap();
    }
}

/// Teclado emprestado ao Python: nada é digitado enquanto vale, a entrada fica na fila e sai uma vez depois.
#[tokio::test]
async fn rust_queue_waits_during_keyboard_loan() {
    let f=Fixture::new().await; let h=f.start();
    let loan=h.control("loan-1".into(),"keyboard_loan".into(),json!({"seconds":30})).await.unwrap();
    assert_eq!(serde_json::to_value(loan.disposition).unwrap(),"accepted");
    let loan_id=loan.payload["loan_id"].as_str().unwrap().to_string();
    let busy=h.control("loan-2".into(),"keyboard_loan".into(),json!({"seconds":30})).await.unwrap();
    assert_eq!(busy.payload["code"],"keyboard_busy","um empréstimo por vez");
    let again=h.control("loan-1".into(),"keyboard_loan".into(),json!({"seconds":30})).await.unwrap();
    assert_eq!(again.payload["loan_id"],loan_id.as_str(),"o mesmo pedido repetido recebe o mesmo empréstimo");
    let wrong=h.control("return-0".into(),"keyboard_return".into(),json!({"loan_id":"loan:1:999"})).await.unwrap();
    assert_eq!(wrong.payload["code"],"keyboard_loan_expired","devolução de outro empréstimo não solta o teclado");
    let reply=h.command(f.command("during","Durante o empréstimo")).await.unwrap();
    assert_eq!(reply.payload["code"],"keyboard_loan");
    tokio::time::sleep(Duration::from_millis(100)).await;     // vários ciclos do drenador
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada digitado durante o empréstimo");
    let back=h.control("return-1".into(),"keyboard_return".into(),json!({"loan_id":loan_id})).await.unwrap();
    assert_eq!(serde_json::to_value(back.disposition).unwrap(),"accepted");
    let typed=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.iter().any(|a|a.contains("Durante o empréstimo"))).count();
    f.wait_for("digitação depois do empréstimo",||typed()>=1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(typed(),1,"a entrada sai uma vez");
    h.stop().await.unwrap();
}

/// Prazo vencido: o Rust retoma o teclado sozinho e a devolução tardia é recusada com código.
#[tokio::test]
async fn keyboard_loan_expires_and_rust_takes_it_back() {
    let f=Fixture::new().await; let h=f.start();
    let loan=h.control("loan-1".into(),"keyboard_loan".into(),json!({"seconds":1})).await.unwrap();
    let loan_id=loan.payload["loan_id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let reply=h.command(f.command("after","Depois do prazo")).await.unwrap();
    assert_ne!(reply.payload["code"],"keyboard_loan");
    f.wait_for("entrega depois do prazo",||f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==true)).await;
    let late=h.control("return-1".into(),"keyboard_return".into(),json!({"loan_id":loan_id})).await.unwrap();
    assert_eq!(serde_json::to_value(late.disposition).unwrap(),"rejected");
    assert_eq!(late.payload["code"],"keyboard_loan_expired");
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_deferred_without_write_backs_off_and_surfaces_the_reason() {
    let f=Fixture::new().await; *f.io.ghost.lock().unwrap()="rascunho".into();
    let h=f.start_full(broadcast::channel(128).0,Duration::from_millis(150));
    assert_eq!(h.command(f.command("busy","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    let start=std::time::Instant::now();
    while h.snapshot().await.unwrap()["view"]["input_stalled"]!="composer_busy" {assert!(start.elapsed()<WAIT,"o motivo não chegou à vista"); tokio::time::sleep(Duration::from_millis(5)).await;}
    f.wait_for("motivo chegou ao canal da sessão", || f.status.borrow().input_stalled.as_deref()==Some("composer_busy")).await;
    // Parada visível, as tentativas seguem espaçadas: o tique de 15 ms daria ~30 em 500 ms.
    let clears=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.last().is_some_and(|a|a=="C-s")).count();
    let before=clears(); tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(clears()-before<=6,"tentativas sem espera crescente: {}",clears()-before);
    assert!(h.snapshot().await.unwrap()["error"].is_null(),"a fila não pode parar como erro");
    f.io.ghost.lock().unwrap().clear();
    f.wait_for("entregue depois do rascunho sair",||f.state()["rows"][0]["delivered"]==true).await;
    let start=std::time::Instant::now();
    while !h.snapshot().await.unwrap()["view"]["input_stalled"].is_null() {assert!(start.elapsed()<WAIT,"o motivo não saiu da vista"); tokio::time::sleep(Duration::from_millis(5)).await;}
    f.wait_for("entrega limpou o canal da sessão", || f.status.borrow().input_stalled.is_none()).await;
    h.stop().await.unwrap();
}
// Tique de 15 ms dobrando: aos 2,5 s a próxima tentativa só viria perto de 3,8 s.
#[tokio::test]
async fn terminal_runtime_stall_retries_as_soon_as_the_composer_empties() {
    let f=Fixture::new().await; *f.io.ghost.lock().unwrap()="rascunho".into();
    let h=f.start_full(broadcast::channel(128).0,Duration::from_secs(30));
    assert_eq!(h.command(f.command("busy","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    f.io.ghost.lock().unwrap().clear();
    let start=std::time::Instant::now();
    f.wait_for("entregue depois do rascunho sair",||f.state()["rows"][0]["delivered"]==true).await;
    assert!(start.elapsed()<Duration::from_millis(800),"o composer vazio esperou a série: {:?}",start.elapsed());
    h.stop().await.unwrap();
}
#[tokio::test]
async fn terminal_runtime_stall_belongs_to_the_line_not_the_session() {
    let f=Fixture::new().await; *f.io.ghost.lock().unwrap()="rascunho".into();
    let h=f.start_full(broadcast::channel(128).0,Duration::from_secs(30));
    assert_eq!(h.command(f.command("busy","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    // B entra atrás de A: a fila não esvazia entre um e outro.
    h.queue("append-B".into(),Action::Append {text:"B".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("B".into())}).await.unwrap();
    let first=f.state()["rows"][0]["id"].as_str().unwrap().to_string();
    let stashes=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.last().is_some_and(|a|a=="C-s")).count();
    let before=stashes();
    h.queue("abandon-A".into(),Action::Abandon {entry_id:first.clone()}).await.unwrap();
    h.queue("remove-A".into(),Action::Remove {entry_id:first}).await.unwrap();
    let start=std::time::Instant::now();
    f.wait_for("a linha nova tenta sem herdar a espera",||stashes()>before).await;
    assert!(start.elapsed()<Duration::from_millis(800),"a linha nova herdou a espera da apagada: {:?}",start.elapsed());
    h.stop().await.unwrap();
}
#[tokio::test(flavor="multi_thread")]
async fn terminal_runtime_restart_during_a_deferral_without_write_requeues_and_delivers_once() {
    let f=Fixture::new().await; *f.io.ghost.lock().unwrap()="rascunho".into();
    // Os fatos (uma captura) passam; segura a leitura do composer, já com o despacho começado.
    f.io.hold_skip.store(1,std::sync::atomic::Ordering::Release); f.io.hold_capture.store(true,std::sync::atomic::Ordering::Release);
    // O backend cai com o ator lendo o composer: nada foi escrito no pane.
    let dying=tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let handle={let _inside=dying.enter(); f.start()};
    let command=f.command("ola-id","ola");
    dying.spawn(async move {let _=handle.command(command).await;});
    f.wait_for("despacho começou",||f.state()["operations"]["ola-id"]["status"]=="dispatching").await;
    dying.shutdown_background();
    let start=std::time::Instant::now();
    while queue::acquire_lease(&f.target.lease_path).is_err() {assert!(start.elapsed()<WAIT,"a trava do ator morto não saiu"); tokio::time::sleep(Duration::from_millis(5)).await;}
    f.io.hold_capture.store(false,std::sync::atomic::Ordering::Release); f.io.ghost.lock().unwrap().clear();
    let h=f.start();
    f.wait_for("entregue depois do reinício",||f.state()["rows"][0]["confirmed"]==true || f.io.calls.lock().unwrap().iter().any(|r|r.args.contains(&"-l".into()) && r.args.last().is_some_and(|a|a=="ola"))).await;
    let state=f.state();
    assert!(state["runtime_state"]["terminal_write_barrier"].is_null(),"adiamento sem escrita não pode virar trava");
    assert_ne!(state["operations"]["ola-id"]["status"],"unknown");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let typed=f.io.calls.lock().unwrap().iter().filter(|r|r.args.contains(&"-l".into()) && r.args.last().is_some_and(|a|a=="ola")).count();
    assert_eq!(typed,1,"a mensagem sai uma vez");
    h.stop().await.unwrap();
}


/// Bem longe: o teste não depende do prazo.
fn far()->std::time::Instant {std::time::Instant::now()+Duration::from_secs(30)}

/// Operação de mod pelo executor: serial com a entrada, fora do diário, recusada com o teclado emprestado.
#[tokio::test]
async fn pane_operations_skip_the_journal_and_respect_the_loan() {
    let f=Fixture::new().await; let h=f.start();
    // O retrato espera o ator terminar a recuperação do início, que grava no diário.
    h.snapshot().await.unwrap();
    let before=f.state()["operations"].as_object().map_or(0,|o|o.len());
    assert_eq!(h.pane(PaneOp::Mouse{row:0,col:104},far()).await.unwrap(),PaneReply::Done);
    assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(s) if s.contains('❯')));
    let sent:Vec<Vec<String>>=f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="send-keys").map(|r|r.args.clone()).collect();
    assert_eq!(sent,vec![vec!["send-keys".to_string(),"-t".into(),"%1".into(),"-l".into(),"--".into(),"\u{1b}[<0;105;1M\u{1b}[<0;105;1m".into()]]);
    assert_eq!(f.state()["operations"].as_object().map_or(0,|o|o.len()),before,"operação de mod não entra no diário");
    let loan=h.control("loan-1".into(),"keyboard_loan".into(),json!({"seconds":30})).await.unwrap();
    assert_eq!(serde_json::to_value(loan.disposition).unwrap(),"accepted");
    assert_eq!(h.pane(PaneOp::Mouse{row:0,col:104},far()).await.unwrap_err().code,"keyboard_loan");
    assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap_err().code,"keyboard_loan","não reserva o pane emprestado");
}

/// Operação que chegou à vez dela depois do ponto de partida não age: o app já ouviu que o clique falhou.
#[tokio::test]
async fn pane_operation_past_its_start_does_not_run() {
    let f=Fixture::new().await; let h=f.start();
    let late=std::time::Instant::now();
    assert_eq!(h.pane(PaneOp::Mouse{row:0,col:104},late).await.unwrap_err().code,"mods_deadline");
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada chega ao pane");
}

/// O `Release` atrasado (limpeza do clique depois do prazo) solta na hora: a fila não fica guardada até
/// o fim da reserva.
#[tokio::test]
async fn a_late_release_still_frees_the_pane() {
    let f=Fixture::new().await; let h=f.start();
    assert_eq!(h.pane(PaneOp::Hold{millis:10_000},far()).await.unwrap(),PaneReply::Done);
    let past=std::time::Instant::now()-Duration::from_secs(1);
    assert_eq!(h.pane(PaneOp::Release,past).await.unwrap(),PaneReply::Done,"soltar vale sempre");
    let started=std::time::Instant::now();
    let command={let h=h.clone();let command=f.command("depois","Depois do soltar");tokio::spawn(async move {h.command(command).await})};
    let waited=f.entered("depois",started).await;
    assert!(waited<Duration::from_secs(5),"o comando esperou a reserva de 10 s: {waited:?}");
    command.await.unwrap().unwrap();
    f.wait_for("entrega depois do soltar",||f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==true)).await;
    h.stop().await.unwrap();
}

/// Com o pane reservado ao clique de mod, nem a fila nem um comando escrevem nele; o que chegou sai na
/// ordem depois do `Release`.
#[tokio::test]
async fn mods_hold_parks_writes_until_release() {
    let f=Fixture::new().await;
    f.idle.store(false,std::sync::atomic::Ordering::Release); f.ready.store(false,std::sync::atomic::Ordering::Release);
    let h=f.start();
    h.command(f.command("fila","Na fila")).await.unwrap();
    assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap(),PaneReply::Done);
    f.idle.store(true,std::sync::atomic::Ordering::Release); f.ready.store(true,std::sync::atomic::Ordering::Release);
    let direct={let h=h.clone();let command=f.command("direto","Durante o clique");tokio::spawn(async move {h.command(command).await})};
    tokio::time::sleep(Duration::from_millis(150)).await;     // dez ciclos do relógio de 15 ms
    assert!(!direct.is_finished(),"o comando espera o clique soltar o pane");
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada digitado com o pane reservado");
    assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)),"o clique segue lendo o pane");
    assert_eq!(h.pane(PaneOp::Release,far()).await.unwrap(),PaneReply::Done);
    direct.await.unwrap().unwrap();
    // A linha vira entregue antes de digitar: a prova observa o texto que chegou ao driver.
    f.wait_for("entregas depois do clique",||f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==true)
        && f.sent_prompts()==["Na fila\nDurante o clique"]).await;
    h.stop().await.unwrap();
    assert_eq!(f.sent_prompts(),["Na fila\nDurante o clique"],"o lote sai uma vez e conserva a ordem");
}

/// Sem `Release` (a tarefa do clique sumiu), a reserva vence sozinha no prazo dela.
#[tokio::test]
async fn a_mods_hold_ends_by_itself() {
    let f=Fixture::new().await; let h=f.start();
    // Antes do pedido: a reserva começa depois deste instante, e a espera medida nunca fica menor que ela.
    let started=std::time::Instant::now();
    assert_eq!(h.pane(PaneOp::Hold{millis:200},far()).await.unwrap(),PaneReply::Done);
    let command={let h=h.clone();let command=f.command("depois","Depois da reserva");tokio::spawn(async move {h.command(command).await})};
    let waited=f.entered("depois",started).await;
    assert!(waited>=Duration::from_millis(150),"o comando esperou a reserva vencer: {waited:?}");
    assert!(waited<Duration::from_secs(3),"a reserva vencida soltou o comando logo: {waited:?}");
    command.await.unwrap().unwrap();
    f.wait_for("entrega depois da reserva",||f.state()["rows"].as_array().unwrap().iter().all(|r|r["delivered"]==true)).await;
    h.stop().await.unwrap();
}

/// Posição de cada texto digitado na ordem das chamadas ao multiplexador.
fn typed_at(f:&Fixture,text:&str)->Vec<usize> {
    f.io.calls.lock().unwrap().iter().enumerate().filter(|(_,r)|r.args[0]=="send-keys" && r.args.iter().any(|a|a==text)).map(|(i,_)|i).collect()
}

/// Dois comandos guardados durante o clique saem na ordem em que chegaram, e o que chega com algo ainda
/// guardado entra atrás dele.
#[tokio::test]
async fn mods_hold_releases_parked_commands_in_arrival_order() {
    let f=Fixture::new().await; let h=f.start();
    assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap(),PaneReply::Done);
    let first={let h=h.clone();let command=f.command("primeiro","Primeiro");tokio::spawn(async move {h.command(command).await})};
    tokio::time::sleep(Duration::from_millis(40)).await;
    let second={let h=h.clone();let command=f.command("segundo","Segundo");tokio::spawn(async move {h.command(command).await})};
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!first.is_finished() && !second.is_finished(),"os dois esperam o clique");
    assert_eq!(h.pane(PaneOp::Release,far()).await.unwrap(),PaneReply::Done);
    first.await.unwrap().unwrap(); second.await.unwrap().unwrap();
    let (a,b)=(typed_at(&f,"Primeiro"),typed_at(&f,"Segundo"));
    assert_eq!((a.len(),b.len()),(1,1),"cada um sai uma vez");
    assert!(a[0]<b[0],"o primeiro guardado sai antes do segundo");
    h.stop().await.unwrap();
}

/// A reserva vence sozinha mesmo com o relógio desligado por um erro de manutenção (`receipt_scan`): o
/// comando guardado sai no prazo da reserva, sem esperar outra mensagem.
#[tokio::test]
async fn a_mods_hold_ends_by_itself_with_the_clock_stopped_by_an_error() {
    let f=Fixture::new().await; let h=f.start();
    h.command(f.command("accepted","Olá")).await.unwrap();
    std::fs::remove_file(&f.target.transcript).unwrap(); std::fs::create_dir(&f.target.transcript).unwrap();
    let start=std::time::Instant::now();
    while h.snapshot().await.unwrap()["error"]!="receipt_scan" {assert!(start.elapsed()<WAIT,"o erro não apareceu"); tokio::time::sleep(Duration::from_millis(5)).await;}
    // Antes do pedido: a reserva começa depois deste instante, e a espera medida nunca fica menor que ela.
    let started=std::time::Instant::now();
    assert_eq!(h.pane(PaneOp::Hold{millis:200},far()).await.unwrap(),PaneReply::Done);
    // Comando sem linha na fila: não depende do transcript que o teste quebrou.
    let command={let h=h.clone();let command=f.command("barra","/help");tokio::spawn(async move {h.command(command).await})};
    let waited=f.entered("barra",started).await;
    assert!(waited<Duration::from_secs(3),"o comando guardado ficou esperando outra mensagem: {waited:?}");
    assert!(waited>=Duration::from_millis(150),"o comando esperou a reserva vencer: {waited:?}");
    tokio::time::timeout(WAIT,command).await.expect("o comando guardado não respondeu").unwrap().ok();
    h.stop().await.unwrap();
}

/// Parada durante a reserva: quem estava guardado ouve que o ator está parando.
#[tokio::test]
async fn stop_during_a_mods_hold_answers_runtime_stopping() {
    let f=Fixture::new().await; let h=f.start();
    assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap(),PaneReply::Done);
    let parked={let h=h.clone();let command=f.command("guardado","Guardado");tokio::spawn(async move {h.command(command).await})};
    let drain={let h=h.clone();tokio::spawn(async move {h.drain().await})};
    tokio::time::sleep(Duration::from_millis(40)).await;
    h.stop().await.unwrap();
    assert_eq!(parked.await.unwrap().err().map(|e|e.code).as_deref(),Some("runtime_stopping"));
    assert_eq!(drain.await.unwrap().unwrap_err().code,"runtime_stopping");
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada digitado");
}

const RULE_80:&str="────────────────────────────────────────────────────────────────────────────────";

/// Foco na faixa de um mod que a leitura reconhece sem a âncora: a faixa recolhida, em inverso.
fn band_focus_screen()->String {format!("Resposta do Claude\n\x1b[7m plugin panel hidden \x1b[0m\n{RULE_80}\n❯ \n{RULE_80}\n")}

/// Um realce do próprio Claude Code logo acima do prompt, sem mod na tela, não segura a mensagem.
#[tokio::test]
async fn an_inverse_above_the_prompt_without_a_mod_does_not_defer() {
    let f=Fixture::new().await;
    *f.io.mods_screen.lock().unwrap()=Some(format!("Resposta do Claude\n\x1b[7m opção selecionada \x1b[0m  outra opção\n{RULE_80}\n❯ \n{RULE_80}\n"));
    let h=f.start();
    let reply=h.command(f.command("realce","Com um realce na tela")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Accepted,"{:?}",reply.payload);
    assert_eq!(typed_at(&f,"Com um realce na tela").len(),1);
    h.stop().await.unwrap();
}

/// Foco no painel ao lado: a borda `│` na cor do foco até a régua do prompt.
fn pane_focus_screen()->String {
    let border="\x1b[38;2;177;185;249m│\x1b[0m";
    let left=|text:&str|format!("{text:<50}{border} Painel do mod");
    format!("{}\n{}\n{}\n{}\n❯ \n{}\n",left("Resposta do Claude"),left(""),left(""),&RULE_80[..50*3],&RULE_80[..50*3])
}

/// Com o foco fora do prompt (faixa ou painel de um mod), nada é escrito; a entrada fica na fila e sai
/// sozinha quando o foco volta ao prompt.
async fn focus_away_defers_until_it_returns(screen:String) {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(screen); let h=f.start();
    let reply=h.command(f.command("foco","Com o foco no mod")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(reply.payload["code"],"mods_focus");
    // Espera as releituras em vez de um tempo fixo: no Windows o relógio anda de 15 em 15 ms e 150 ms
    // às vezes davam um tique só.
    let reads=||f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="capture-pane" && !r.args.contains(&"-S".into())).count();
    f.wait_for("a fila tenta de novo a cada tique",||reads()>=2).await;
    assert!(f.io.calls.lock().unwrap().iter().all(|r|r.args[0]!="send-keys"),"nada escrito com o foco fora do prompt");
    // A linha vira `delivered` no `Claim` de cada tique e volta no adiamento: o que vale é não desistir e
    // a tela ser relida a cada tentativa.
    assert_ne!(f.state()["rows"][0]["desistiu"],true,"a linha continua na fila");
    *f.io.mods_screen.lock().unwrap()=None;
    f.wait_for("entrega com o foco de volta",||!typed_at(&f,"Com o foco no mod").is_empty()).await;
    h.stop().await.unwrap();
    assert_eq!(typed_at(&f,"Com o foco no mod").len(),1,"sai uma vez");
}

#[tokio::test]
async fn focus_on_the_mods_band_defers_the_queue() {focus_away_defers_until_it_returns(band_focus_screen()).await;}

#[tokio::test]
async fn focus_on_a_mods_pane_defers_the_queue() {focus_away_defers_until_it_returns(pane_focus_screen()).await;}

/// A faixa inteira de um mod, com um botão em inverso (o foco que a pessoa levou com `ctrl+x tab`).
fn full_band_focus_screen()->String {format!("Resposta do Claude\nRevisão do MR  \x1b[7m[ Abrir ]\x1b[0m  [ Fechar ]\n{RULE_80}\n❯ \n{RULE_80}\n")}

/// A faixa inteira focada só é reconhecida pela âncora do mod: com ela, a escrita espera; sem ela, o
/// inverso acima do prompt é tratado como realce do próprio Claude Code e não segura a mensagem.
#[tokio::test]
async fn the_full_band_focused_defers_the_queue_with_the_mods_anchor() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(full_band_focus_screen()); let h=f.start();
    *h.anchor().lock().unwrap()=Some("Revisão do MR".into());
    let reply=h.command(f.command("faixa","Com a faixa focada")).await.unwrap();
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(reply.payload["code"],"mods_focus");
    tokio::time::sleep(Duration::from_millis(150)).await;     // dez ciclos do relógio de 15 ms
    assert!(typed_at(&f,"Com a faixa focada").is_empty(),"nada escrito com o botão da faixa focado");
    // Sem a âncora (nenhum mod na tela), o mesmo inverso não segura a entrega.
    *h.anchor().lock().unwrap()=None;
    f.wait_for("entrega sem a âncora",||!typed_at(&f,"Com a faixa focada").is_empty()).await;
    h.stop().await.unwrap();
}

/// Dentro da reserva de um clique de mod o pane é conferido uma vez: no psmux cada conferência é mais um
/// processo, e a reserva por teclado com uma dúzia de botões na faixa estourava o prazo. Fora dela, cada
/// operação confere de novo.
#[tokio::test]
async fn within_a_mods_hold_the_pane_is_checked_once() {
    let f=Fixture::new().await; let h=f.start();
    let checks=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.last().is_some_and(|a|a.starts_with("#{session_name}"))).count();
    let before=checks();
    assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap(),PaneReply::Done);
    for _ in 0..3 {
        assert!(matches!(h.pane(PaneOp::Formats,far()).await.unwrap(),PaneReply::Formats(_)));
        assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)));
        assert_eq!(h.pane(PaneOp::Keys(vec!["C-x".into(),"Tab".into()]),far()).await.unwrap(),PaneReply::Done);
    }
    assert_eq!(checks()-before,1,"uma conferência na reserva inteira");
    assert_eq!(h.pane(PaneOp::Release,far()).await.unwrap(),PaneReply::Done);
    assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)));
    assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)));
    assert_eq!(checks()-before,3,"fora da reserva, cada operação confere");
    h.stop().await.unwrap();
}

/// Com o foco devolvido pelo `ctrl+x tab`, a entrada parada pelo foco num mod sai depois de `focus_return`.
async fn focus_left_on_a_mod_is_returned(screen:String,anchor:Option<&str>) {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(screen);
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(300));
    *h.anchor().lock().unwrap()=anchor.map(str::to_owned);
    let started=std::time::Instant::now();
    let reply=h.command(f.command("preso","Foco esquecido no mod")).await.unwrap();
    assert_eq!(reply.payload["code"],"mods_focus");
    f.wait_for("entrega depois da devolução do foco",||!typed_at(&f,"Foco esquecido no mod").is_empty()).await;
    assert!(started.elapsed()>=Duration::from_millis(300),"a entrada esperou o prazo antes de mexer no foco");
    assert!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst)>=1,"o foco voltou pelo ctrl+x tab");
    h.stop().await.unwrap();
    assert_eq!(typed_at(&f,"Foco esquecido no mod").len(),1);
}

/// Foco num botão da faixa com um painel aberto ao lado: a borda do painel apagada, o inverso na faixa.
fn band_focus_with_pane_screen()->String {
    let left=|text:&str|format!("{text:<50}│ Painel do mod");
    // O preenchimento conta os caracteres do texto visível, não os da sequência de escape.
    let band=format!("Revisão do MR  \x1b[7m[ Abrir ]\x1b[0m{}│ Painel do mod"," ".repeat(50-24));
    format!("{}\n{}\n{band}\n{}\n❯ \n{}\n",left("Resposta do Claude"),left(""),&RULE_80[..50*3],&RULE_80[..50*3])
}

/// A pessoa levou o foco à faixa no terminal, com um painel aberto, e saiu: a mensagem do app não fica
/// parada.
#[tokio::test]
async fn the_users_own_focus_on_the_band_with_a_pane_is_returned() {
    focus_left_on_a_mod_is_returned(band_focus_with_pane_screen(),Some("Revisão do MR")).await;
}

/// Só com a faixa na tela o `ctrl+x tab` gira dentro dela e não volta ao prompt: nenhuma tecla, a entrada
/// espera a pessoa.
#[tokio::test]
async fn with_only_the_band_no_key_is_sent() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(full_band_focus_screen());
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(100));
    *h.anchor().lock().unwrap()=Some("Revisão do MR".into());
    assert_eq!(h.command(f.command("faixa","Só a faixa")).await.unwrap().payload["code"],"mods_focus");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst),0);
    assert!(typed_at(&f,"Só a faixa").is_empty());
    *f.io.mods_screen.lock().unwrap()=None;
    f.wait_for("entrega com o foco de volta",||!typed_at(&f,"Só a faixa").is_empty()).await;
    h.stop().await.unwrap();
}

/// A devolução que não volta ao prompt é tentada duas vezes por linha e depois só registrada: nada de um
/// laço de teclas. A entrada continua na fila até o foco sair do mod.
#[tokio::test]
async fn a_failed_return_is_tried_twice_and_keeps_the_entry() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(100));
    assert_eq!(h.command(f.command("preso","Sem volta")).await.unwrap().payload["code"],"mods_focus");
    let rings=||f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst);
    f.wait_for("a primeira devolução",||rings()>=32).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rings(),32,"a segunda espera o dobro do prazo");
    f.wait_for("a segunda devolução",||rings()>=64).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(rings(),64,"sem terceira");
    assert!(typed_at(&f,"Sem volta").is_empty());
    *f.io.mods_screen.lock().unwrap()=None;
    f.wait_for("entrega com o foco de volta",||!typed_at(&f,"Sem volta").is_empty()).await;
    h.stop().await.unwrap();
}

/// No modo `User` o plugin entrega sem tecla: a guarda do foco não roda (nenhuma leitura da tela dos mods) e
/// não atrasa, mesmo com o foco num painel. Com `@` a entrega é `Fill`, aperta `Enter`, e a guarda adia.
#[tokio::test]
async fn the_focus_guard_runs_only_when_the_delivery_presses_keys() {
    for (text,guarded) in [("Sem tecla",false),("Com @arquivo",true)] {
        let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
        f.unknown.store(true,std::sync::atomic::Ordering::Release);
        let h=f.start();
        let mods_reads=||f.io.calls.lock().unwrap().iter().filter(|r|r.args[0]=="capture-pane" && !r.args.contains(&"-S".into())).count();
        let reply=h.command(f.command("entrada",text)).await.unwrap();
        assert_eq!(reply.payload["code"]=="mods_focus",guarded,"{text}: {:?}",reply.payload);
        assert_eq!(mods_reads()>0,guarded,"{text}: a tela dos mods só é lida quando a entrega aperta tecla");
        h.stop().await.unwrap();
    }
}


/// Os modos das publicações ao plugin, na ordem.
fn published(f:&Fixture)->Vec<String> {
    f.plugin_requests.lock().unwrap().iter().map(|value|value["modo"].as_str().unwrap().to_owned()).collect()
}

/// Com o plugin vivo, o foco esquecido num mod volta pelo pedido a ele, sem o `ctrl+x tab`: também com só a
/// faixa na tela, onde o anel nunca chega ao prompt. Depois a entrada segue ao plugin (`Fill`).
async fn the_plugin_returns_the_focus(screen:String,anchor:Option<&str>) {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(screen);
    f.unknown.store(true,std::sync::atomic::Ordering::Release);
    f.focus_plugin.store(true,std::sync::atomic::Ordering::Release);
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(300));
    *h.anchor().lock().unwrap()=anchor.map(str::to_owned);
    assert_eq!(h.command(f.command("preso","Com @foco no mod")).await.unwrap().payload["code"],"mods_focus");
    f.wait_for("a entrada vai ao plugin depois da devolução",||published(&f).contains(&"fill".to_string())).await;
    assert_eq!(published(&f),["focus","fill"]);
    assert_eq!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst),0,"nenhum ctrl+x tab");
    h.stop().await.unwrap();
}

#[tokio::test]
async fn the_plugin_returns_the_focus_from_a_pane() {the_plugin_returns_the_focus(pane_focus_screen(),None).await;}

#[tokio::test]
async fn the_plugin_returns_the_focus_with_only_the_band() {the_plugin_returns_the_focus(full_band_focus_screen(),Some("Revisão do MR")).await;}

/// A espera crescente da entrada parada não passa da hora da devolução: com o composer que não parece livre
/// (o painel ao lado), as tentativas saíam em 1, 3, 7 e 15 s, e a devolução dos 10 s esperava a de 15.
#[tokio::test]
async fn the_stall_backoff_does_not_delay_the_focus_return() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
    *f.io.ghost.lock().unwrap()="rascunho".into();
    f.unknown.store(true,std::sync::atomic::Ordering::Release);
    f.focus_plugin.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(4000));
    let started=std::time::Instant::now();
    assert_eq!(h.command(f.command("preso","Com @foco no mod")).await.unwrap().payload["code"],"mods_focus");
    f.wait_for("a devolução pelo plugin",||published(&f).contains(&"focus".to_string())).await;
    let waited=started.elapsed();
    // As tentativas da espera crescente caem perto de 1,07 s, 2 s, 3,9 s e 7,7 s. Marcada para 4 s, logo depois
    // de uma delas, a devolução sai perto de 4,04 s; sem a correção, esperava a de 7,7 s (7,83 s medidos). O teto
    // de 6,3 s deixa 2,2 s para a carga de todos os testes juntos (o runner do Windows chegou a atrasar 1,3 s).
    assert!(waited>=Duration::from_millis(4000) && waited<Duration::from_millis(6300),"devolução em {waited:?}");
    h.stop().await.unwrap();
}

/// A volta do clique pede ao plugin uma vez por reserva, e só com ele vivo: sem isso cada volta da limpeza
/// pagaria o diário e a espera do aviso antes do anel.
#[tokio::test]
async fn the_click_asks_the_plugin_once_per_hold_and_only_when_it_is_live() {
    let f=Fixture::new().await; let h=f.start();
    let hold=||h.pane(PaneOp::Hold{millis:5000},far());
    assert_eq!(hold().await.unwrap(),PaneReply::Done);
    assert!(h.pane(PaneOp::ReturnFocus,far()).await.is_err(),"sem plugin vivo");
    assert!(published(&f).is_empty(),"nada publicado sem plugin");
    assert_eq!(h.pane(PaneOp::Release,far()).await.unwrap(),PaneReply::Done);
    f.unknown.store(true,std::sync::atomic::Ordering::Release);f.start_plugin();
    assert_eq!(hold().await.unwrap(),PaneReply::Done);
    assert!(h.pane(PaneOp::ReturnFocus,far()).await.is_err(),"o plugin não conhece o pedido");
    assert!(h.pane(PaneOp::ReturnFocus,far()).await.is_err());
    assert!(published(&f).is_empty(),"poll sem a capacidade focus não recebe publicação");
    assert_eq!(f.state()["operations"].as_object().unwrap().values().filter(|operation|operation["payload"]["kind"]=="focus").count(),1);
    assert_eq!(h.pane(PaneOp::Release,far()).await.unwrap(),PaneReply::Done);
    f.focus_plugin.store(true,std::sync::atomic::Ordering::Release);f.start_plugin();
    assert_eq!(hold().await.unwrap(),PaneReply::Done);
    assert_eq!(h.pane(PaneOp::ReturnFocus,far()).await.unwrap(),PaneReply::Done,"reserva nova, nova tentativa");
    assert_eq!(published(&f),["focus"]);
    h.stop().await.unwrap();
}

/// O plugin que não conhece o pedido (`not_written`) deixa a volta ao `ctrl+x tab`.
#[tokio::test]
async fn without_the_plugin_focus_the_ring_returns_it() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
    f.unknown.store(true,std::sync::atomic::Ordering::Release);
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(300));
    assert_eq!(h.command(f.command("preso","Com @foco no mod")).await.unwrap().payload["code"],"mods_focus");
    f.wait_for("a entrada vai ao plugin depois do anel",||published(&f).contains(&"fill".to_string())).await;
    assert_eq!(published(&f),["fill"],"sem focus o poll só recebe entrada");
    assert!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst)>=1);
    h.stop().await.unwrap();
}

/// A limpeza de um clique desistiu com o teclado num painel e a reserva venceu sozinha: a mensagem guardada
/// durante o clique não fica presa.
#[tokio::test]
async fn a_cleanup_that_gave_up_does_not_leave_the_message_stuck() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(300));
    assert_eq!(h.pane(PaneOp::Hold{millis:200},far()).await.unwrap(),PaneReply::Done);
    let parked={let h=h.clone();let command=f.command("guardada","Guardada no clique");tokio::spawn(async move {h.command(command).await})};
    f.wait_for("entrega depois da reserva e da devolução",||!typed_at(&f,"Guardada no clique").is_empty()).await;
    parked.await.unwrap().unwrap();
    assert!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst)>=1);
    h.stop().await.unwrap();
}


/// A espera do foco é da linha: uma linha que saiu da fila com o foco num painel não deixa a seguinte
/// devolver o foco na hora.
#[tokio::test]
async fn the_focus_wait_starts_over_for_the_next_row() {
    let f=Fixture::new().await; *f.io.mods_screen.lock().unwrap()=Some(pane_focus_screen());
    let h=f.start_returning(broadcast::channel(128).0,Duration::from_secs(30),Duration::from_millis(600));
    assert_eq!(h.command(f.command("primeira","Primeira")).await.unwrap().payload["code"],"mods_focus");
    // Até o `Abandon` chegar, o relógio tenta a primeira de novo; no runner Windows as gravações do diário
    // passam dos 600 ms e ela pode devolver o foco antes. Sem `ring_returns` o foco fica no painel.
    h.queue("abandona".into(),Action::Abandon {entry_id:"primeira".into()}).await.unwrap();
    let rung=f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(f.io.ring_keys.load(std::sync::atomic::Ordering::SeqCst),rung,"a linha abandonada não devolve o foco");
    f.io.ring_returns.store(true,std::sync::atomic::Ordering::Release);
    let started=std::time::Instant::now();
    assert_eq!(h.command(f.command("segunda","Segunda")).await.unwrap().payload["code"],"mods_focus");
    f.wait_for("entrega da segunda",||!typed_at(&f,"Segunda").is_empty()).await;
    assert!(started.elapsed()>=Duration::from_millis(600),"a segunda esperou o prazo dela: {:?}",started.elapsed());
    h.stop().await.unwrap();
}


/// Cada reserva confere o pane uma vez, também a renovação: a conferência não vale por todas.
#[tokio::test]
async fn each_mods_hold_checks_the_pane_again() {
    let f=Fixture::new().await; let h=f.start();
    let checks=||f.io.calls.lock().unwrap().iter().filter(|r|r.args.last().is_some_and(|a|a.starts_with("#{session_name}"))).count();
    let before=checks();
    for _ in 0..2 {
        assert_eq!(h.pane(PaneOp::Hold{millis:5000},far()).await.unwrap(),PaneReply::Done);
        assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)));
        assert!(matches!(h.pane(PaneOp::Screen,far()).await.unwrap(),PaneReply::Screen(_)));
    }
    assert_eq!(checks()-before,2,"uma conferência por reserva");
    h.stop().await.unwrap();
}

fn projection(f:&Fixture)->Vec<Value> {
    queue::read_replaced_to_string(&f.target.projection_dir.join("session.jsonl")).unwrap_or_default().lines().map(|l|serde_json::from_str(l).unwrap()).collect()
}

#[tokio::test]
async fn direct_send_publishes_row_only_once_delivered() {
    // Em idle a linha não aparece na vista enquanto os fatos são lidos: a primeira versão publicada já vem entregue.
    let f=Fixture::new().await;
    *f.io.projection_at_facts.lock().unwrap()=Some((f.target.projection_dir.join("session.jsonl"),vec![]));
    let h=f.start();
    assert_eq!(h.command(f.command("direct","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let seen=f.io.projection_at_facts.lock().unwrap().as_ref().unwrap().1.clone();
    // Os fatos do `execute` são lidos entre a gravação da linha e o despacho: a primeira leitura não vê a
    // linha, e nenhuma a vê ainda não entregue.
    assert!(!seen.is_empty() && !seen[0].contains("\"direct\""),"vista publicada antes do despacho: {seen:?}");
    assert!(!seen.iter().any(|view|view.contains("\"delivered\":false")),"linha publicada antes de entregue: {seen:?}");
    let rows=projection(&f);
    assert_eq!(rows.len(),1); assert_eq!(rows[0]["id"],"direct"); assert_eq!(rows[0]["delivered"],true);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn deferred_send_publishes_queued_row() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    assert_eq!(h.command(f.command("wait","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    let rows=projection(&f);
    assert_eq!(rows.len(),1); assert_eq!(rows[0]["id"],"wait"); assert_eq!(rows[0]["delivered"],false);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn held_projection_is_restored_on_restart() {
    // Queda entre a gravação e o despacho: a linha está no diário e a abertura seguinte a publica.
    let f=Fixture::new().await;
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let mut store=store;
    store.exec_held(1,"held",ClockSample {monotonic_s:0.0,epoch_s:2.0},Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("held".into())},true).unwrap();
    assert!(projection(&f).is_empty());
    drop(store);
    Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let rows=projection(&f); assert_eq!(rows.len(),1); assert_eq!(rows[0]["delivered"],false);
}

#[tokio::test]
async fn new_message_waits_behind_older_deferred_row() {
    let f=Fixture::new().await; f.ready.store(false,std::sync::atomic::Ordering::Release); let h=f.start();
    let first=h.command(f.command("A","Primeira")).await.unwrap();
    assert_eq!(first.disposition,hangar_server::runtime::protocol::Disposition::Deferred); assert!(first.payload["code"].is_null());
    let second=h.command(f.command("B","Segunda")).await.unwrap();
    assert_eq!(second.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    assert_eq!(second.payload["code"],"queue_ahead"); assert_eq!(second.payload["queued"],true);
    f.ready.store(true,std::sync::atomic::Ordering::Release);
    f.wait_for("as duas partes no mesmo prompt",||f.sent_prompts()==["Primeira\nSegunda"]).await;
    assert_eq!(f.sent_prompts(),["Primeira\nSegunda"]);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn delivered_row_confirms_when_transcript_grows_without_waiting_the_tick() {
    let f=Fixture::new().await;
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let options=TerminalOptions {io:f.io.clone(),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    assert_eq!(h.command(f.command("fast","Olá")).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    std::fs::write(&f.target.transcript,"{\"type\":\"user\",\"uuid\":\"1\",\"message\":{\"content\":\"Olá\"}}\n").unwrap();
    let start=std::time::Instant::now();
    f.wait_for("confirmada pelo transcript",||f.state()["rows"][0]["confirmed"]==true).await;
    assert!(start.elapsed()<Duration::from_secs(2),"confirmação esperou {:?}",start.elapsed());
    h.stop().await.unwrap();
}

/// Parity A2: rodapé que não muda depois da tecla não deixa a sessão doente. O Python relia o modo
/// e devolvia o final; a entrega seguia.
#[tokio::test]
async fn unchanged_footer_after_mode_key_reads_back_and_keeps_delivering() {
    let f=Fixture::new().await;
    *f.io.panes.lock().unwrap()=vec![format!("{}\n❯ \n{}\n⏵⏵ accept edits on (shift+tab to cycle)\n","─".repeat(32),"─".repeat(32))];
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let memory=Arc::new(Mutex::new(hangar_server::state::permission::PermissionMemory::default()));
    let terms=Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig::default()));
    let options=TerminalOptions {io:f.io.clone(),permissions:Some(memory.clone()),terms:Some(terms),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    let reply=h.control("mode".into(),"set_permission_mode".into(),json!({"mode":"plan"})).await.unwrap();
    assert_eq!(reply.payload["current"],"acceptEdits","{reply:?}");
    assert_eq!(reply.disposition,hangar_server::runtime::protocol::Disposition::Rejected,"{reply:?}");
    let snapshot=h.snapshot().await.unwrap();
    assert!(snapshot["error"].is_null(),"modo incerto não trava a entrega: {snapshot}");
    assert_eq!(memory.lock().unwrap().confirmed("sid").unwrap().map(|v|v.current),Some("acceptEdits".into()));
    h.stop().await.unwrap();
}

/// Pane cujo rodapé mostra o modo corrente e cada BTab passa ao seguinte do ciclo.
struct CycleIo {inner:Arc<Io>,modes:Mutex<std::collections::VecDeque<&'static str>>,btabs:std::sync::atomic::AtomicUsize}
impl TerminalIo for CycleIo {
    fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput> { Box::pin(async move {
        if r.args[0]=="send-keys" && r.args.last().is_some_and(|k|k=="BTab") {
            self.btabs.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            let mut modes=self.modes.lock().unwrap();let first=modes.pop_front().unwrap();modes.push_back(first);
            return Ok(CommandOutput {success:true,stdout:vec![]});
        }
        let capture=r.args[0]=="capture-pane";
        let mut out=self.inner.command(r).await?;
        if capture {
            let phrase=match *self.modes.lock().unwrap().front().unwrap() {"manual"=>"manual mode on","auto"=>"auto mode on","plan"=>"plan mode on",_=>"accept edits on"};
            out.stdout.extend(format!("⏵⏵ {phrase} (shift+tab to cycle)\n").into_bytes());
        }
        Ok(out)
    }) }
    fn socket<'a>(&'a self,m:&'a NativeMessage,e:Vec<u8>)->IoFuture<'a,WriteOutcome> {self.inner.socket(m,e)}
}

/// Parity M2 (`test_perm_get_cache_usa_mesma_lista`): a lista sondada fica guardada na vida da conversa;
/// a leitura seguinte a devolve e a sonda seguinte não dá outra volta de teclas.
#[tokio::test]
async fn probed_modes_are_cached_for_reads_and_later_probes() {
    let f=Fixture::new().await;
    let io=Arc::new(CycleIo {inner:f.io.clone(),modes:Mutex::new(["manual","auto","plan"].into()),btabs:Default::default()});
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let memory=Arc::new(Mutex::new(hangar_server::state::permission::PermissionMemory::default()));
    let terms=Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig::default()));
    let options=TerminalOptions {io:io.clone(),permissions:Some(memory),terms:Some(terms),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    let probe=h.control("p1".into(),"permission_modes".into(),json!({"probe":true})).await.unwrap();
    assert_eq!(probe.payload["modes"],json!(["manual","auto","plan"]),"{probe:?}");
    let pressed=io.btabs.load(std::sync::atomic::Ordering::SeqCst);
    let read=h.control("r1".into(),"permission_modes".into(),json!({"probe":false})).await.unwrap();
    assert_eq!(read.payload["modes"],json!(["manual","auto","plan"]),"{read:?}");
    let again=h.control("p2".into(),"permission_modes".into(),json!({"probe":true})).await.unwrap();
    assert_eq!(again.payload["modes"],json!(["manual","auto","plan"]),"{again:?}");
    assert_eq!(io.btabs.load(std::sync::atomic::Ordering::SeqCst),pressed,"a segunda sonda usa a lista guardada");
    h.stop().await.unwrap();
}

/// Parity M3 (`test_bypass_fora_do_ciclo_com_sessao_trabalhando_recusa_sem_reabrir`): bypass fora do ciclo
/// do processo vai direto à reabertura, sem girar o modo; a ociosidade recusa com o motivo do Python.
#[tokio::test]
async fn bypass_outside_the_cycle_presses_no_key_and_busy_says_why() {
    let f=Fixture::new().await;
    let io=Arc::new(CycleIo {inner:f.io.clone(),modes:Mutex::new(["manual","auto","plan"].into()),btabs:Default::default()});
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let memory=Arc::new(Mutex::new(hangar_server::state::permission::PermissionMemory::default()));
    let terms=Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig::default()));
    let options=TerminalOptions {io:io.clone(),permissions:Some(memory),terms:Some(terms),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    // O agente deste teste é o próprio processo de teste: nasceu sem a flag de bypass.
    let reply=h.control("bypass".into(),"set_permission_mode".into(),json!({"mode":"bypassPermissions"})).await.unwrap();
    assert_eq!((reply.disposition,reply.payload["requires_reopen"].clone(),reply.payload["cycle_closed"].clone()),
        (hangar_server::runtime::protocol::Disposition::Deferred,json!(true),json!(true)),"{reply:?}");
    assert_eq!(io.btabs.load(std::sync::atomic::Ordering::SeqCst),0,"sem girar o modo");
    f.idle.store(false,std::sync::atomic::Ordering::Release);
    let busy=h.control("guard".into(),"lifecycle_guard".into(),json!({})).await.unwrap_err();
    assert_eq!(busy.code,"erro_sessao_trabalhando");
    h.stop().await.unwrap();
}

/// Parity B1 (`test_perm_get_sonda_que_nao_voltou_marca_restaurado_false`): sonda que não volta ao modo
/// inicial responde 200 com a lista e `restaurado:false`, não 409 sem a lista.
#[tokio::test]
async fn probe_that_does_not_restore_still_answers_the_list() {
    let f=Fixture::new().await;
    let cycle=["manual","auto","plan","auto","plan","auto","plan","auto","plan","auto"];
    let io=Arc::new(CycleIo {inner:f.io.clone(),modes:Mutex::new(cycle.into()),btabs:Default::default()});
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let memory=Arc::new(Mutex::new(hangar_server::state::permission::PermissionMemory::default()));
    let terms=Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig::default()));
    let options=TerminalOptions {io:io.clone(),permissions:Some(memory),terms:Some(terms),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    let h=TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    let probe=h.control("p".into(),"permission_modes".into(),json!({"probe":true})).await.unwrap();
    assert_eq!((probe.disposition,probe.payload["restaurado"].clone(),probe.payload["modes"].clone()),
        (hangar_server::runtime::protocol::Disposition::Accepted,json!(false),json!(["manual","auto","plan"])),"{probe:?}");
    h.stop().await.unwrap();
}

// ── Seletor do `/model` no ator ─────────────────────────────────────────────────────────────────────
/// As capturas da tela (sem `-e`) saem do roteiro do golden, a última fica; as da guarda e o resto vão ao
/// `Io` da fixture. As teclas só são anotadas.
struct PickerIo {inner:Arc<Io>,script:Mutex<std::collections::VecDeque<String>>,keys:Mutex<Vec<String>>}
impl TerminalIo for PickerIo {
    fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput> { Box::pin(async move {
        if r.args[0]=="send-keys" {
            let key=r.args.last().unwrap().clone();
            self.keys.lock().unwrap().push(if key=="\r" {"Enter".into()} else {key});
            return Ok(CommandOutput {success:true,stdout:vec![]});
        }
        if r.args[0]=="capture-pane" && !r.args.contains(&"-e".into()) {
            let mut script=self.script.lock().unwrap();
            let pane=if script.len()>1 {script.pop_front().unwrap()} else {script[0].clone()};
            return Ok(CommandOutput {success:true,stdout:pane.into_bytes()});
        }
        self.inner.command(r).await
    }) }
    fn socket<'a>(&'a self,m:&'a NativeMessage,e:Vec<u8>)->IoFuture<'a,WriteOutcome> {self.inner.socket(m,e)}
}
fn picker_golden(name:&str)->Value {
    let path=concat!(env!("CARGO_MANIFEST_DIR"),"/../../backend/tests/fixtures/contract/golden/model_picker.json");
    let data:Value=serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    data["cases"].as_array().unwrap().iter().find(|case|case["name"]==name).unwrap().clone()
}
fn picker_actor(f:&Fixture,case:&Value)->(Arc<PickerIo>,hangar_server::runtime::terminal::TerminalHandle) {
    let script=case["panes"].as_array().unwrap().iter().map(|p|p.as_str().unwrap().to_owned()).collect();
    let io=Arc::new(PickerIo {inner:f.io.clone(),script:Mutex::new(script),keys:Default::default()});
    let lease=crate::lease_when_free(&f.target.lease_path);
    let store=Store::open(&f.target.state_path,&f.target.projection_dir,queue::State::new("key",1,"session",vec![])).unwrap();
    let terms=Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig::default()));
    let options=TerminalOptions {io:io.clone(),terms:Some(terms),limits:InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:1,ready_attempts:1,cleanup_attempts:1},tick:Duration::from_secs(600),..TerminalOptions::default()};
    (io,TerminalActor::spawn(f.target.clone(),QueueActor::start(store,lease),f.policy.clone(),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0))))
}

/// B20: o sucesso sai do ator com o JSON do Python e fica no diário como aceito.
#[tokio::test]
async fn model_picker_b20_success_answers_python_json_and_is_journaled() {
    let f=Fixture::new().await;
    let case=picker_golden("session_navigates_and_presses_s");
    let (io,h)=picker_actor(&f,&case);
    let reply=h.control("me".into(),"set_model_effort".into(),json!({"model":"sonnet","effort":null,"scope":"session"})).await;
    assert_eq!(*io.keys.lock().unwrap(),["/model","Enter","Down","s"]);
    let reply=reply.unwrap();
    assert_eq!((reply.disposition,&reply.payload),(hangar_server::runtime::protocol::Disposition::Accepted,&case["expect"]["ok"]));
    assert_eq!(f.state()["operations"]["me"]["status"],"accepted");
    h.stop().await.unwrap();
}

/// B10/B11: com o teclado emprestado o seletor não digita, e a recusa tem o código do Python.
#[tokio::test]
async fn model_picker_b10_b11_borrowed_keyboard_refuses_with_code() {
    let f=Fixture::new().await;
    let (io,h)=picker_actor(&f,&picker_golden("session_navigates_and_presses_s"));
    assert_eq!(h.control("loan".into(),"keyboard_loan".into(),json!({"seconds":30})).await.unwrap().disposition,hangar_server::runtime::protocol::Disposition::Accepted);
    let refused=h.control("me".into(),"set_model_effort".into(),json!({"model":"sonnet","effort":null,"scope":"session"})).await;
    assert!(io.keys.lock().unwrap().is_empty(),"nenhuma tecla com o teclado emprestado");
    assert_eq!(refused.map_err(|e|e.code).map(|r|r.disposition),Err("erro_opcao_nao_convergiu".into()));
    h.stop().await.unwrap();
}

/// B11: o mesmo id repetido responde o recibo do diário (o resultado, enxugado como todo recibo), sem
/// digitar de novo.
#[tokio::test]
async fn model_picker_b11_repeated_id_answers_from_the_journal() {
    let f=Fixture::new().await;
    let (io,h)=picker_actor(&f,&picker_golden("session_navigates_and_presses_s"));
    let payload=json!({"model":"sonnet","effort":null,"scope":"session"});
    let first=h.control("me".into(),"set_model_effort".into(),payload.clone()).await;
    let typed=io.keys.lock().unwrap().len();
    let again=h.control("me".into(),"set_model_effort".into(),payload).await;
    assert_eq!(io.keys.lock().unwrap().len(),typed,"a repetição não digita");
    assert_eq!(again.map(|r|r.disposition).ok(),first.map(|r|r.disposition).ok());
    assert!(typed>0);
    h.stop().await.unwrap();
}

#[tokio::test]
async fn queued_row_goes_out_at_turn_end_and_is_confirmed_without_any_drain_or_confirm_request() {
    // Sem hub nem aparelho ninguém pede `drain` ou `confirm`: o relógio do ator entrega no fim do turno e
    // confirma pelo transcript, também depois de reiniciar com a linha esperando.
    use std::sync::atomic::Ordering::Release;
    let f=Fixture::new().await; f.idle.store(false,Release);
    let h=f.start();
    h.queue("append-a".into(),Action::Append {text:"fim do turno".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("a".into())}).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(f.sent_prompts().is_empty(),"turno em andamento: nada é escrito");
    f.idle.store(true,Release);
    f.wait_for("entregue no fim do turno",||f.sent_prompts()==vec!["fim do turno".to_string()]).await;
    std::fs::write(&f.target.transcript,format!("{}\n",json!({"type":"user","sessionId":"sid","uuid":"a-echo","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"fim do turno"}}))).unwrap();
    f.wait_for("confirmada pelo transcript",||f.state()["rows"][0]["confirmed"]==true).await;
    f.idle.store(false,Release);
    h.queue("append-b".into(),Action::Append {text:"depois do reinicio".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("b".into())}).await.unwrap();
    h.stop().await.unwrap();
    f.idle.store(true,Release);
    let h=f.start();
    f.wait_for("entregue depois do reinício",||f.sent_prompts().last().map(String::as_str)==Some("depois do reinicio")).await;
    h.stop().await.unwrap();
}

