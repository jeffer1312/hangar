use hangar_server::terminal_input::*;
use std::{collections::VecDeque, sync::{Arc, Mutex}, time::Duration};
fn screen(s: &str) -> String { format!("history\n{}\n❯ {s}\n{}\n⏵⏵ bypass permissions\n\n", "─".repeat(30), "─".repeat(30)) }
fn binding() -> TerminalBinding { TerminalBinding { name: "test".into(), pane: "%7".into(), conversation: "uuid".into(), generation: 1, created: 17, mux_argv: vec!["fake".into(), "-L".into(), "isolated".into()], windows: false, clipboard_lock_path: None } }
struct Services { facts: Mutex<InputFacts>, reply: Mutex<PluginReply>, published: Mutex<Vec<PluginRequest>> }
impl Services { fn new() -> Self { Self { facts: Mutex::new(InputFacts { binding: binding(), ready: true, idle: true, open_question: false, plugin_live: false, plugin_user: false,clipboard_available:true, native: None, screen: None }), reply: Mutex::new(PluginReply::Unavailable), published: Mutex::new(vec![]) } } }
impl TerminalServices for Services {
 fn facts<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> { Box::pin(async { Ok(self.facts.lock().unwrap().clone()) }) }
 fn publish<'a>(&'a self, _: &'a TerminalBinding, r: PluginRequest) -> ServiceFuture<'a, PluginReply> { Box::pin(async move { self.published.lock().unwrap().push(r); Ok(self.reply.lock().unwrap().clone()) }) }
 fn permission_guard<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}
struct FakeIo { screens: Mutex<VecDeque<String>>, calls: Mutex<Vec<CommandRequest>>, socket: Mutex<WriteOutcome>, envelope: Mutex<Vec<u8>>, fail: Mutex<Option<String>>, formats: Mutex<String>, clients: Mutex<String>, attached: Mutex<String> }
impl FakeIo {
 fn new(s: Vec<String>) -> Self { Self { screens: Mutex::new(s.into()), calls: Mutex::new(vec![]), socket: Mutex::new(WriteOutcome::NotWritten), envelope: Mutex::new(vec![]), fail: Mutex::new(None), formats: Mutex::new("1|1|0|150|45\n".into()), clients: Mutex::new(String::new()), attached: Mutex::new("0\n".into()) } }
 fn writes(&self) -> Vec<CommandRequest> { self.calls.lock().unwrap().iter().filter(|r| r.args.iter().any(|a| a == "send-keys" || a == "paste-buffer")).cloned().collect() }
}
impl TerminalIo for FakeIo {
 fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
 self.calls.lock().unwrap().push(r.clone());
 if r.args.iter().any(|a| a == "display-message") && r.args.last().is_some_and(|a| a.contains("mouse_sgr_flag")) { return Ok(CommandOutput { success: true, stdout: self.formats.lock().unwrap().clone().into_bytes() }); }
 if r.args.iter().any(|a| a == "display-message") && r.args.last().is_some_and(|a| a == "#{session_attached}") { return Ok(CommandOutput { success: true, stdout: self.attached.lock().unwrap().clone().into_bytes() }); }
 if r.args.iter().any(|a| a == "list-clients") { return Ok(CommandOutput { success: true, stdout: self.clients.lock().unwrap().clone().into_bytes() }); }
 if r.args.iter().any(|a| a == "display-message") { return Ok(CommandOutput { success: true, stdout: b"test\t%7\t17\n".to_vec() }); }
 if r.args.iter().any(|a| a == "capture-pane") { let mut s = self.screens.lock().unwrap(); let t = if s.len()>1 {s.pop_front().unwrap()} else {s.front().cloned().unwrap_or_default()}; return Ok(CommandOutput {success: true, stdout:t.into_bytes()}); }
 if self.fail.lock().unwrap().as_ref().is_some_and(|k| r.args.last()==Some(k)) {return Err(IoFailure {code:"partial",may_have_written:true});}
 Ok(CommandOutput {success:true,stdout:vec![]}) }) }
 fn socket<'a>(&'a self, _: &'a NativeMessage, envelope: Vec<u8>) -> IoFuture<'a, WriteOutcome> {Box::pin(async move {*self.envelope.lock().unwrap()=envelope;Ok(*self.socket.lock().unwrap())})}
}
fn driver<S:TerminalServices+'static>(io:Arc<FakeIo>,s:Arc<S>)->TerminalDriver {TerminalDriver::new(binding(),s,io,InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3})}
#[test]
fn terminal_input_composer_proof() {
 let old=ComposerSnapshot::parse(&screen("[Pasted text #1 +12 lines] [Image #1]")).unwrap();
 assert_eq!(old.proves("long original message",&old),Proof::Absent);
 for s in ["[Pasted text #2 +2 lines]","[Image #2]","a long ori\nginal mes sage"] {assert_eq!(ComposerSnapshot::parse(&screen(s)).unwrap().proves("a long original message",&old),Proof::Present);}
 assert_eq!(ComposerSnapshot::parse(&format!("a long original message\n{}",screen(""))).unwrap().proves("a long original message",&old),Proof::Absent);
 assert!(ComposerSnapshot::parse("no composer").is_none());
 let empty=ComposerSnapshot::parse(&screen("")).unwrap();
 assert_eq!(ComposerSnapshot::parse(&screen("/clear │ [name]")).unwrap().proves("/clear",&empty),Proof::Present);
 assert_eq!(ComposerSnapshot::parse(&screen("/compact <optional custom summarization instructions>")).unwrap().proves("/compact",&empty),Proof::Present);
 assert_eq!(ComposerSnapshot::parse(&screen("/compact <a> <b>")).unwrap().proves("/compact",&empty),Proof::Unreadable);
 assert_eq!(ComposerSnapshot::parse(&screen("/clear [x] [name]")).unwrap().proves("/clear",&empty),Proof::Unreadable);
 assert_eq!(ComposerSnapshot::parse(&screen("/clear [a #1]")).unwrap().proves("/clear",&empty),Proof::Unreadable);
 let draft=ComposerSnapshot::parse(&screen("/clear [nota]")).unwrap();
 assert_eq!(draft.proves("/clear",&draft),Proof::Unreadable);
 assert_eq!(ComposerSnapshot::parse(&screen("/compact <a #1>")).unwrap().proves("/compact",&empty),Proof::Unreadable);
 let draft=ComposerSnapshot::parse(&screen("/compact <nota>")).unwrap();
 assert_eq!(draft.proves("/compact",&draft),Proof::Unreadable);
}
#[test]
fn terminal_input_composer_below_long_agent_panel() {
 let agents: String = (0..6).map(|i| format!("  ◯ general-purpose  task {i}\n")).collect();
 for main in ["  ● main", "  ❯ ● main"] {
  let s = format!("{}\n  model │ folder\n\n{main}\n{agents}\n", screen("draft").trim_end_matches('\n'));
  assert_eq!(ComposerSnapshot::parse(&s).unwrap().content, "draft");
 }
 let steps: String = (0..9).map(|i| format!("● step {i}\n")).collect();
 assert!(ComposerSnapshot::parse(&format!("{}{steps}", screen(""))).is_none());
 // Opções `◯` de um diálogo sem a linha `main` não são o painel.
 let options: String = (0..9).map(|i| format!("  ◯ option {i}\n")).collect();
 assert!(ComposerSnapshot::parse(&format!("{}{options}", screen(""))).is_none());
}
#[tokio::test]
async fn terminal_input_short_text_literal_cr() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("ok"),screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("ok","id").await;
 assert_eq!(r.disposition,Disposition::Accepted);
 assert_eq!(io.writes().len(),2); assert_eq!(io.writes()[1].args.last().unwrap(),"\r");
 assert!(io.writes()[1].args.contains(&"-l".into()));
}
#[tokio::test]
async fn terminal_input_multiline_stdin_placeholder() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("[Pasted text #1 +2 lines]"),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("first\nsecond","id").await.disposition,Disposition::Accepted);
 let c=io.calls.lock().unwrap(); let load=c.iter().find(|r|r.args.contains(&"load-buffer".into())).unwrap(); assert_eq!(load.stdin,b"first\nsecond");
}
#[tokio::test]
async fn terminal_input_validation_and_stale_guard() {
 for mode in ["control","overlay","stale"] {let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());if mode=="overlay"{s.facts.lock().unwrap().open_question=true;}if mode=="stale"{s.facts.lock().unwrap().binding.conversation="other".into();}
 assert_ne!(driver(io.clone(),s).prompt(if mode=="control"{"x\r"}else{"hello"},"id").await.disposition,Disposition::Accepted);assert!(io.writes().is_empty());}
}
#[tokio::test]
async fn terminal_input_plugin_unknown_never_retypes() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().plugin_live=true;*s.reply.lock().unwrap()=PluginReply::Unknown;
 assert_eq!(driver(io.clone(),s).prompt("long original message","id").await.disposition,Disposition::Unknown);assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_slash_bypasses_plugin() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("/clear"),screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().plugin_live=true;s.facts.lock().unwrap().plugin_user=true;*s.reply.lock().unwrap()=PluginReply::Filled;
 assert_eq!(driver(io.clone(),s.clone()).prompt("/clear","id").await.disposition,Disposition::Accepted);
 assert!(s.published.lock().unwrap().is_empty());assert_eq!(io.writes().len(),2);
}
#[tokio::test]
async fn terminal_input_enter_uncertain_never_retries() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("long original message"),screen("")]));*io.fail.lock().unwrap()=Some("\r".into());
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await;
 assert_eq!(r.disposition,Disposition::Unknown);assert_eq!(r.stage,DeliveryStage::Submit);assert_eq!(io.writes().len(),2);
}
#[tokio::test]
async fn terminal_input_partial_cleanup_only_owned_without_internal_retry() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("long original message"),screen(""),screen("long original message"),screen("")]));*io.fail.lock().unwrap()=Some("long original message".into());
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await;
 assert_eq!(r.disposition,Disposition::Deferred);assert_eq!(r.cleanup,Cleanup::Proved);assert_eq!(io.writes().iter().filter(|r|r.args.last().is_some_and(|a|a=="long original message")).count(),1);
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("foreign message")]));*io.fail.lock().unwrap()=Some("long original message".into());
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await.disposition,Disposition::Unknown);assert_eq!(io.writes().len(),1);
}
/// Fatos da conversa que somem depois da escrita (o `/pull` do plugin trocando, o transcript fechado):
/// o texto é nosso e o pane é o mesmo, então ele sai do composer e a entrega adia, sem Enter nem trava.
struct LostFacts { inner: Services, io: Arc<FakeIo>, allowed: usize, seen: Mutex<usize> }
impl TerminalServices for LostFacts {
 fn facts<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> { Box::pin(async move {
  if self.io.writes().iter().any(|w| w.args.contains(&"-l".into())) {
   let mut seen = self.seen.lock().unwrap(); *seen += 1;
   if *seen > self.allowed { return Err(ServiceError("terminal_conversation_unavailable")); }
  }
  self.inner.facts(b).await
 }) }
 fn publish<'a>(&'a self, b: &'a TerminalBinding, r: PluginRequest) -> ServiceFuture<'a, PluginReply> { self.inner.publish(b, r) }
 fn permission_guard<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, ()> { self.inner.permission_guard(b) }
 fn life_check<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}
#[tokio::test]
async fn terminal_input_facts_lost_after_write_clean_own_text_and_defer() {
 // 0: a prova da tela já não tem fatos; 1: a prova passa e o Enter não tem.
 for allowed in [0, 1] {
  // Uma tela por captura: antes, a prova (só com fatos), a limpeza e depois do C-u.
  let mut screens=vec![screen(""),screen("long original message")];if allowed==1 {screens.push(screen("long original message"));}screens.push(screen(""));
  let io=Arc::new(FakeIo::new(screens));
  let s=Arc::new(LostFacts {inner:Services::new(),io:io.clone(),allowed,seen:Mutex::new(0)});
  let limits=InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3};
  let r=TerminalDriver::new(binding(),s,io.clone(),limits).prompt("long original message","id").await;
  assert_eq!((r.disposition,r.cleanup),(Disposition::Deferred,Cleanup::Proved),"allowed={allowed}: {r:?}");
  assert_eq!(io.writes().last().unwrap().args.last().unwrap(),"C-u","allowed={allowed}");
  assert!(io.writes().iter().all(|w|w.args.last().is_none_or(|a|a!="\r")),"sem fatos o Enter não sai");
 }
}
/// Campo como o do Claude: o Ctrl+U tira só a linha do cursor (no fim) e, numa linha vazia, só a quebra.
struct LineIo { composer: Mutex<String>, buffer: Mutex<Vec<u8>>, writes: Mutex<Vec<String>>, foreign: Mutex<String> }
impl LineIo { fn new(start: &str) -> Self { Self { composer: Mutex::new(start.into()), buffer: Mutex::new(vec![]), writes: Mutex::new(vec![]), foreign: Mutex::new(String::new()) } } }
impl TerminalIo for LineIo {
 fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
  let out = |s: String| Ok(CommandOutput { success: true, stdout: s.into_bytes() });
  let has = |a: &str| r.args.iter().any(|x| x == a);
  if has("display-message") { return out("test\t%7\t17\n".into()); }
  if has("capture-pane") { return out(screen(&self.composer.lock().unwrap())); }
  if has("load-buffer") { *self.buffer.lock().unwrap() = r.stdin.clone(); return out(String::new()); }
  let mut c = self.composer.lock().unwrap();
  if has("paste-buffer") {
   self.writes.lock().unwrap().push("paste".into());
   // A pessoa digitando ao mesmo tempo: o texto dela entra antes do nosso.
   c.insert_str(0, &self.foreign.lock().unwrap());
   c.push_str(&String::from_utf8(self.buffer.lock().unwrap().clone()).unwrap());
   return out(String::new());
  }
  if has("send-keys") {
   let key = r.args.last().unwrap().clone();
   self.writes.lock().unwrap().push(key.clone());
   match key.as_str() {
    "C-u" => if c.ends_with('\n') { c.pop(); } else { let cut = c.rfind('\n').map_or(0, |n| n + 1); c.truncate(cut); },
    "\r" => c.clear(),
    _ if has("-l") => c.push_str(&key),
    _ => {}
   }
  }
  out(String::new())
 }) }
 fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> { Box::pin(async { Ok(WriteOutcome::NotWritten) }) }
}
/// Fatos que somem na `allowed`-ésima consulta depois de o texto entrar no campo.
struct DroppingFacts { inner: Services, io: Arc<LineIo>, allowed: usize, seen: Mutex<usize> }
impl TerminalServices for DroppingFacts {
 fn facts<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> { Box::pin(async move {
  if !self.io.writes.lock().unwrap().is_empty() {
   let mut seen = self.seen.lock().unwrap(); *seen += 1;
   if *seen > self.allowed { return Err(ServiceError("terminal_conversation_unavailable")); }
  }
  self.inner.facts(b).await
 }) }
 fn publish<'a>(&'a self, b: &'a TerminalBinding, r: PluginRequest) -> ServiceFuture<'a, PluginReply> { self.inner.publish(b, r) }
 fn permission_guard<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, ()> { self.inner.permission_guard(b) }
 fn life_check<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}
#[tokio::test]
async fn terminal_input_multiline_text_leaves_the_field_whole_when_facts_drop_before_enter() {
 let text = "[de: filha] linha um\nlinha dois\nlinha tres";
 // Os fatos caem em cada ponto entre o texto entrar e o Enter, inclusive na conferência da própria tecla.
 for allowed in 0..6 {
  let io = Arc::new(LineIo::new(""));
  let s = Arc::new(DroppingFacts { inner: Services::new(), io: io.clone(), allowed, seen: Mutex::new(0) });
  let r = TerminalDriver::new(binding(), s, io.clone(), instant_limits()).prompt(text, "id").await;
  let writes = io.writes.lock().unwrap().clone();
  // Enter já enviado: a dúvida depois dele continua incerta (pode ter chegado), e o campo não fica com texto.
  if writes.iter().any(|w| w == "\r") { assert_eq!(*io.composer.lock().unwrap(), "", "allowed={allowed}"); continue; }
  assert_eq!((r.disposition, r.cleanup), (Disposition::Deferred, Cleanup::Proved), "allowed={allowed}: {r:?} {writes:?}");
  assert_eq!(*io.composer.lock().unwrap(), "", "allowed={allowed}: o campo fica vazio");
 }
}
#[tokio::test]
async fn terminal_input_cleanup_stops_at_text_that_is_not_ours() {
 // A pessoa digitou junto com o nosso texto: o campo já não é só nosso, e nada é apagado.
 let io = Arc::new(LineIo::new(""));
 *io.foreign.lock().unwrap() = "texto da pessoa\n".into();
 let s = Arc::new(DroppingFacts { inner: Services::new(), io: io.clone(), allowed: 0, seen: Mutex::new(0) });
 let r = TerminalDriver::new(binding(), s, io.clone(), instant_limits()).prompt("linha um\nlinha dois", "id").await;
 assert_ne!(r.disposition, Disposition::Accepted);
 assert!(!io.writes.lock().unwrap().iter().any(|w| w == "C-u" || w == "\r"), "{:?}", io.writes.lock().unwrap());
 assert_eq!(*io.composer.lock().unwrap(), "texto da pessoa\nlinha um\nlinha dois", "nunca apaga o que não é nosso");
}
#[tokio::test]
async fn terminal_input_socket_partial_never_falls_back() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().native=Some(NativeMessage {socket:"/private/socket".into(),origin:"uds:/inbox".into(),sender:"peer".into(),mode:"bypass".into(),message_id:Some("uuid-message".into())});*io.socket.lock().unwrap()=WriteOutcome::Unknown;
 let r=driver(io.clone(),s).prompt("[de: peer] long message","id").await;
 assert_eq!(r.disposition,Disposition::Unknown);assert!(io.writes().is_empty());assert_eq!(r.message_id.as_deref(),Some("uuid-message"));
 let envelope:serde_json::Value=serde_json::from_slice(&io.envelope.lock().unwrap()).unwrap();
 assert_eq!(envelope,serde_json::json!({"msgV":1,"msg_id":"uuid-message","type":"user","message":{"role":"user","content":"<cross-session-message from=\"uds:/inbox\" from-name=\"peer\" from-mode=\"bypass\">\n[de: peer] long message\n</cross-session-message>"},"priority":"next","from":"uds:/inbox"}));
}
#[tokio::test]
async fn terminal_input_allowlists_explicit_steer() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));let d=driver(io.clone(),Arc::new(Services::new()));
 assert_eq!(d.key("C-c",false).await.disposition,Disposition::Rejected);assert_eq!(d.key("C-c",true).await.disposition,Disposition::Accepted);assert_eq!(d.key("PageUp",false).await.disposition,Disposition::Accepted);let steer=d.steer().await;assert_eq!((steer.disposition,steer.code.as_str()),(Disposition::Deferred,"no_queued_input"),"sem a marca não há prova de consumo");assert_eq!(io.writes().len(),2);
}
#[tokio::test]
async fn terminal_input_select_closed_loop() {
 let p=|row|format!("Question\n{}\nEsc to cancel · to navigate",if row==1{"❯ 1. first\n  2. second"}else{"  1. first\n❯ 2. second"});
 let io=Arc::new(FakeIo::new(vec![p(1),p(2),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).select(2,true).await.disposition,Disposition::Accepted);assert_eq!(io.writes()[0].args.last().unwrap(),"Down");assert_eq!(io.writes()[1].args.last().unwrap(),"\r");
}
fn option_answer(index: usize, label: &str) -> QuestionAnswer { QuestionAnswer {kind:AnswerKind::Option,question_id:None,indices:vec![index],labels:vec![label.into()],multi:false,value:None,type_index:None,chat_index:None} }
fn picker() -> String {"Question\n❯ 1. first\n  2. second\nEsc to cancel · to navigate".into()}
#[tokio::test]
async fn terminal_input_plugin_fill_proves_then_enters() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("hello @peer"),screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().plugin_live=true;*s.reply.lock().unwrap()=PluginReply::Filled;
 assert_eq!(driver(io.clone(),s.clone()).prompt("hello @peer","id").await.disposition,Disposition::Accepted);
 assert_eq!(s.published.lock().unwrap()[0].mode,PluginMode::Fill);assert_eq!(io.writes().len(),1);
}
#[tokio::test]
async fn terminal_input_plugin_fill_enters_when_long_message_makes_composer_unreadable() {
 let tall=screen(&(0..45).map(|i| format!("linha {i} da mensagem longa")).collect::<Vec<_>>().join("\n"));
 assert!(ComposerSnapshot::parse(&tall).is_none(),"composer alto demais para ler");
 assert!(ComposerSnapshot::too_tall(&tall) && !ComposerSnapshot::too_tall("um diálogo sem réguas"));
 let text=(0..45).map(|i| format!("linha {i} da mensagem longa")).collect::<Vec<_>>().join("\n");
 let io=Arc::new(FakeIo::new(vec![screen(""),tall,screen("")]));let s=Arc::new(Services::new());
 s.facts.lock().unwrap().plugin_live=true;*s.reply.lock().unwrap()=PluginReply::Filled;
 let r=driver(io.clone(),s.clone()).prompt(&text,"id").await;
 assert_eq!(r.disposition,Disposition::Accepted,"o Filled do plugin basta");
 assert_eq!(s.published.lock().unwrap()[0].mode,PluginMode::Fill);assert_eq!(io.writes().len(),1,"só o Enter");
}
#[tokio::test]
async fn terminal_input_plugin_fill_never_enters_on_a_screen_without_composer() {
 let io=Arc::new(FakeIo::new(vec![screen(""),"Diálogo qualquer\n❯ Sim\n  Não".into()]));let s=Arc::new(Services::new());
 s.facts.lock().unwrap().plugin_live=true;*s.reply.lock().unwrap()=PluginReply::Filled;
 let r=driver(io.clone(),s.clone()).prompt("uma mensagem qualquer","id").await;
 assert_ne!(r.disposition,Disposition::Accepted);assert!(io.writes().is_empty(),"nenhum Enter");
}
#[test]
fn terminal_input_composer_of_a_long_message_still_reads() {
 let text=(0..20).map(|i| format!("linha {i} da mensagem longa")).collect::<Vec<_>>().join("\n");
 let now=ComposerSnapshot::parse(&screen(&text)).expect("vinte linhas ainda são o composer");
 assert_eq!(now.proves(&text,&ComposerSnapshot::parse(&screen("")).unwrap()),Proof::Present);
}
#[tokio::test]
async fn terminal_input_plugin_user_requires_capability_idle_and_safe_text() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().plugin_live=true;s.facts.lock().unwrap().plugin_user=true;*s.reply.lock().unwrap()=PluginReply::Accepted;
 assert_eq!(driver(io.clone(),s.clone()).prompt("hello","id").await.disposition,Disposition::Accepted);assert_eq!(s.published.lock().unwrap()[0].mode,PluginMode::User);assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_plugin_user_refusal_sends_text_and_enter_once() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen(""),screen("pode continuar"),screen("")]));
 let services=Arc::new(Services::new());
 services.facts.lock().unwrap().plugin_live=true;
 services.facts.lock().unwrap().plugin_user=true;
 *services.reply.lock().unwrap()=PluginReply::NotWritten;
 let result=driver(io.clone(),services.clone()).prompt("pode continuar","refused").await;
 assert_eq!(services.published.lock().unwrap()[0].mode,PluginMode::User);
 assert_eq!(result.disposition,Disposition::Accepted);
 let writes=io.writes();
 assert_eq!(writes.len(),2,"recusa explícita permite uma escrita e um Enter");
 assert_eq!(writes[0].args.last().unwrap(),"pode continuar");
 assert_eq!(writes[1].args.last().unwrap(),"\r");
}
#[test]
fn terminal_input_review_matches_per_question_not_cross_question_substrings() {
 let a=vec![option_answer(0,"first"),option_answer(1,"second")];
 assert!(review_matches("Review\n→ first\n→ second",&a));assert!(!review_matches("Review\n→ second\n→ first",&a));assert!(!review_matches("Review\n→ firstly\n→ second",&a));
}
#[test]
fn terminal_input_review_ignores_free_text_and_arrows_in_the_history() {
 // Como o Python: só os rótulos de opção se conferem; texto livre com vírgula ou quebra não segura o envio.
 let text=QuestionAnswer {kind:AnswerKind::Text,question_id:None,indices:vec![],labels:vec![],multi:false,value:Some("sim, pode".into()),type_index:Some(2),chat_index:None};
 let a=vec![option_answer(0,"first"),text];
 assert!(review_matches("Review\n→ first\n→ sim, pode\n❯ 1. Submit answers",&a));
 assert!(review_matches("● passo → feito\nReview\n→ first\n→ sim, pode\n❯ 1. Submit answers",&a));
 assert!(!review_matches("Review\n→ outra\n→ sim, pode\n❯ 1. Submit answers",&a));
 // O texto livre é conferido pelo começo que a linha mostra: outro texto não passa.
 assert!(!review_matches("Review\n→ first\n→ não\n❯ 1. Submit answers",&a));
 let long=QuestionAnswer {kind:AnswerKind::Text,question_id:None,indices:vec![],labels:vec![],multi:false,value:Some("uma resposta que quebra em duas linhas".into()),type_index:Some(2),chat_index:None};
 assert!(review_matches("Review\n→ first\n→ uma resposta que quebra\n  em duas linhas\n❯ 1. Submit answers",&[option_answer(0,"first"),long]));
}
#[tokio::test]
async fn terminal_input_answer_reaches_a_picker_whose_footer_the_task_list_pushed_up() {
 let pane=include_str!("../../../../backend/tests/fixtures/pane_askuserquestion.txt").to_string();
 let io=Arc::new(FakeIo::new(vec![pane,screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).answer(&[option_answer(0,"Opção Alpha")]).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"answered"));
 assert_eq!(io.writes().iter().map(|w|w.args.last().unwrap().clone()).collect::<Vec<_>>(),["\r"]);
}
#[tokio::test]
async fn terminal_input_owner_controls_only_check_the_pane() {
 // Claude caído e o pane de volta ao shell: os fatos da sessão falham, mas Parar e o espelho digitam.
 let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().binding.conversation="other".into();
 assert_ne!(driver(io.clone(),s.clone()).key("Escape",false).await.disposition,Disposition::Accepted,"a entrega comum segue a guarda inteira");
 let d=driver(io.clone(),s).controls();
 assert_eq!(d.key("Escape",false).await.disposition,Disposition::Accepted);
 assert_eq!(d.text("claude -c").await.disposition,Disposition::Accepted);
 assert_eq!(d.interrupt(false).await.disposition,Disposition::Accepted);
}
/// Telas reais do Claude Code 2.1.296 com um subagente vendo o composer (`Message @general-purpose…`).
const SUB_COMPOSER: &str = include_str!("../fixtures/subagent/sub_composer.ansi");
#[tokio::test]
async fn terminal_input_composer_of_a_subagent_goes_back_to_main_before_typing() {
 let at=|name:&str|std::fs::read_to_string(format!("{}/tests/fixtures/subagent/{name}.ansi",env!("CARGO_MANIFEST_DIR"))).unwrap();
 let io=Arc::new(FakeIo::new(vec![at("sub_composer"),at("sub_composer"),at("sub_pill"),at("sub_main_cursor"),at("main_view_focus"),at("main_composer2"),screen("hello"),screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("hello","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"submitted"));
 assert_eq!(io.writes().iter().map(|w|w.args.last().unwrap().clone()).collect::<Vec<_>>(),["Down","Down","\r","Escape","hello","\r"]);
 // O foco já voltou ao composer depois do Enter no `main`: sem Esc, que ali interromperia o turno.
 let io=Arc::new(FakeIo::new(vec![at("sub_composer"),at("sub_composer"),at("sub_pill"),at("sub_main_cursor"),at("main_composer2"),at("main_composer2"),screen("hello"),screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("hello","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"submitted"));
 assert_eq!(keys(&io),["Down","Down","\r","hello","\r"]);
}
#[tokio::test]
async fn terminal_input_subagent_composer_with_a_draft_defers_without_typing() {
 let draft=SUB_COMPOSER.replace("\u{1b}[2mMessage @general-purpose…\u{1b}[0m","rascunho do dono");
 assert!(draft.contains("rascunho do dono"));
 let io=Arc::new(FakeIo::new(vec![draft]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("hello","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Deferred,"composer_targets_subagent"));
 assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_multiline_term_input_is_pasted_as_a_block_without_enter() {
 // Decisão do dono: várias linhas no espelho vão coladas em bloco; nada roda até ele apertar Enter.
 let io=Arc::new(FakeIo::new(vec![screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).controls().text("ls\necho fim").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"text_written"));
 let calls=io.calls.lock().unwrap();
 assert_eq!(calls.iter().find(|c|c.args.contains(&"load-buffer".into())).unwrap().stdin,b"ls\necho fim");
 assert!(calls.iter().any(|c|c.args.contains(&"paste-buffer".into()) && c.args.contains(&"-p".into())));
 assert!(!calls.iter().any(|c|c.args.contains(&"send-keys".into())),"nenhuma tecla, nem Enter");
}
#[tokio::test]
async fn terminal_input_plugin_question_by_keys_checks_every_step() {
 use hangar_server::interactions::{ExpectedInteraction,InteractionBinding,InteractionKind,plugin_target};
 let questions=serde_json::json!([{"question":"Cor?","header":"C","options":[{"label":"Azul"},{"label":"Verde"}]},{"question":"Pasta?","header":"P","options":[{"label":"A"},{"label":"B"}]}]);
 let expected=ExpectedInteraction {binding:InteractionBinding {key:"k".into(),conversation:"uuid".into(),generation:1},target:plugin_target(InteractionKind::Question,"ask:x"),
  interaction_id:"id".into(),semantic:serde_json::json!({"questions":questions}),source_revision:None};
 let step=|q:&str,a:&str,b:&str|format!("{q}\n❯ 1. {a}\n  2. {b}\n  3. Type something.\nEnter to select · ↑/↓ to navigate · Esc to cancel");
 // A etapa 2 que aparece é de outra pergunta: nenhuma tecla vai para ela.
 let io=Arc::new(FakeIo::new(vec![step("Cor?","Azul","Verde"),step("Cor?","Azul","Verde"),step("Outra?","X","Y")]));
 let answers=vec![option_answer(0,"Azul"),option_answer(0,"A")];
 let r=driver(io.clone(),Arc::new(Services::new())).plugin_ask_keys(&expected,&answers).await;
 assert_eq!(r.code,"erro_interacao_desatualizada");assert_eq!(keys(&io),["\r"]);
 // Rótulo que não é o da opção do plugin: recusa antes da tecla.
 let io=Arc::new(FakeIo::new(vec![step("Cor?","Azul","Verde")]));
 let r=driver(io.clone(),Arc::new(Services::new())).plugin_ask_keys(&expected,&[option_answer(0,"Verde"),option_answer(0,"A")]).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Rejected,"erro_interacao_desatualizada"));assert!(keys(&io).is_empty());
}
#[tokio::test]
async fn terminal_input_select_outcomes_follow_the_python() {
 let p=|row:usize|format!("Question\n{}\nEsc to cancel · to navigate",if row==1{"❯ 1. first\n  2. second"}else{"  1. first\n❯ 2. second"});
 // Cursor que não chega: só setas saíram, a opção não foi enviada (recusa, não incerta).
 let io=Arc::new(FakeIo::new(vec![p(1)]));
 let r=driver(io.clone(),Arc::new(Services::new())).select(2,true).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Rejected,"cursor_drift"));assert!(!keys(&io).contains(&"\r".to_string()));
 // TUI lenta: a primeira leitura depois do Enter ainda mostra o picker, a seguinte não.
 let io=Arc::new(FakeIo::new(vec![p(1),p(1),screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).select(1,true).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"selected"));
 // Múltipla escolha cuja marca não mudou depois das releituras: incerta com o código, nunca sucesso.
 let multi="Quais?\n❯ 1. [ ] A\n  2. [ ] B\nEsc to cancel · to navigate".to_string();
 let io=Arc::new(FakeIo::new(vec![multi]));
 let r=driver(io.clone(),Arc::new(Services::new())).select(1,true).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Unknown,"selection_unchanged"));assert_eq!(keys(&io),["Space"]);
}
#[tokio::test]
async fn terminal_input_checkbox_in_the_history_is_not_a_multiple_choice() {
 let p=|row:usize|format!("● Plano\n  - [ ] tarefa um\n  - [x] tarefa dois\nPermitir?\n{}\nEsc to cancel · to navigate",if row==1{"❯ 1. Yes\n  2. No"}else{"  1. Yes\n❯ 2. No"});
 let io=Arc::new(FakeIo::new(vec![p(1),screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).select(1,true).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"selected"));
 assert_eq!(io.writes()[0].args.last().unwrap(),"\r");
}
#[tokio::test]
async fn terminal_input_answer_single_and_review_mismatch_never_extra_enter() {
 let io=Arc::new(FakeIo::new(vec![picker(),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).answer(&[option_answer(0,"first")]).await.disposition,Disposition::Accepted);assert_eq!(io.writes().len(),1);
 let io=Arc::new(FakeIo::new(vec![picker(),"Review\n→ wrong\nSubmit answers\nEsc to cancel".into()]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).answer(&[option_answer(0,"first")]).await.disposition,Disposition::Unknown);assert_eq!(io.writes().len(),1);
}
#[tokio::test]
async fn terminal_input_submit_selected_needs_submit_tab() {
 let multi="Question\n❯ 1. [✔] first\n  2. [ ] second\nEsc to cancel · to navigate";
 let io=Arc::new(FakeIo::new(vec![multi.into(),multi.into()]));
 // Sem a aba de envio o Enter não sai: só o `Right` foi, nada enviado (recusa, como o Python).
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).submit_selected().await.disposition,Disposition::Rejected);assert_eq!(io.writes().len(),1);assert_eq!(io.writes()[0].args.last().unwrap(),"Right");
}
#[tokio::test]
async fn terminal_input_interrupt_clear_checks_nonempty_to_avoid_rewind() {
 for draft in ["", "draft"] {let io=Arc::new(FakeIo::new(vec![screen(draft)]));assert_eq!(driver(io.clone(),Arc::new(Services::new())).interrupt(true).await.disposition,Disposition::Accepted);assert_eq!(io.writes().len(),if draft.is_empty(){1}else{2});}
}
#[tokio::test]
async fn terminal_input_stale_after_wait_prevents_late_effect() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().ready=false;
 let d=Arc::new(TerminalDriver::new(binding(),s.clone(),io.clone(),InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::from_millis(1),proof_attempts:2,ready_attempts:100,cleanup_attempts:3}));
 let task=tokio::spawn(async move{d.prompt("hello\nsecond","id").await});
 tokio::task::yield_now().await;s.facts.lock().unwrap().binding.generation=2;
 assert_eq!(task.await.unwrap().disposition,Disposition::Deferred);assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_parked_slash_command_gets_one_more_enter() {
 // Como o Python: o mesmo comando parado no composer ganha outro Enter, com ou sem dica de menu.
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("/clear"),screen("/clear"),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("/clear","id").await.disposition,Disposition::Accepted);
 assert_eq!(io.writes().iter().map(|w|w.args.last().unwrap().clone()).collect::<Vec<_>>(),["/clear","\r","\r"]);
 // Nunca um terceiro: comando que segue parado fica incerto.
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("/clear"),screen("/clear")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("/clear","id").await.disposition,Disposition::Unknown);
 assert_eq!(io.writes().len(),3);
}
#[cfg(unix)]
#[tokio::test]
async fn terminal_input_process_cli_reads_stdin_to_eof() {
 let io=ProcessIo::default();let r=io.command(CommandRequest {program:"python3".into(),args:vec!["-c".into(),"import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())".into()],stdin:"ação\\😀\nsecond".as_bytes().to_vec()}).await.unwrap();
 assert!(r.success);assert_eq!(r.stdout,"ação\\😀\nsecond".as_bytes());
}
#[tokio::test]
async fn terminal_input_clipboard_lock_rechecks_generation_after_wait() {
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("clipboard.lock");
 let lock=std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).unwrap();lock.lock().unwrap();
 let mut b=binding();b.windows=true;b.pane="=test:0.0".into();b.clipboard_lock_path=Some(path);
 let s=Arc::new(Services::new());s.facts.lock().unwrap().binding=b.clone();
 let io=Arc::new(FakeIo::new(vec![screen("")]));
 let d=TerminalDriver::new(b,s.clone(),io.clone(),InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::from_millis(1),proof_attempts:2,ready_attempts:100,cleanup_attempts:3});
 let task=tokio::spawn(async move{d.prompt("hello\nsecond","id").await});
 while !io.calls.lock().unwrap().iter().any(|c|c.args.contains(&"capture-pane".into())) {tokio::task::yield_now().await;}
 s.facts.lock().unwrap().binding.generation=2;drop(lock);
 assert_eq!(task.await.unwrap().disposition,Disposition::Deferred);
 assert!(io.writes().is_empty());assert!(!io.calls.lock().unwrap().iter().any(|c|c.program=="powershell.exe"));
}
#[tokio::test]
async fn terminal_input_serial_wait_rechecks_binding_before_control() {
 struct Blocking {inner:FakeIo,entered:tokio::sync::Notify,resume:tokio::sync::Notify}
 impl TerminalIo for Blocking {
  fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput>{Box::pin(async move{if r.args.contains(&"capture-pane".into()){self.entered.notify_one();self.resume.notified().await;}self.inner.command(r).await})}
  fn socket<'a>(&'a self,d:&'a NativeMessage,e:Vec<u8>)->IoFuture<'a,WriteOutcome>{self.inner.socket(d,e)}
 }
 let io=Arc::new(Blocking {inner:FakeIo::new(vec![screen("")]),entered:tokio::sync::Notify::new(),resume:tokio::sync::Notify::new()});let s=Arc::new(Services::new());let d=Arc::new(TerminalDriver::new(binding(),s.clone(),io.clone(),InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3}));
 let capture=tokio::spawn({let d=d.clone();async move{d.capture().await}});io.entered.notified().await;
 let control=tokio::spawn({let d=d.clone();async move{d.key("Enter",false).await}});
 s.facts.lock().unwrap().binding.conversation="other".into();io.resume.notify_one();capture.await.unwrap().unwrap();
 assert_eq!(control.await.unwrap().disposition,Disposition::Deferred);assert!(io.inner.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_windows_clipboard_held_until_proof_and_preserves_unicode() {
 struct Probe {inner:FakeIo,path:std::path::PathBuf,checked:Mutex<bool>}
 impl TerminalIo for Probe {
  fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput>{Box::pin(async move{
   let pasted=self.inner.calls.lock().unwrap().iter().any(|c|c.args.last().is_some_and(|s|s=="M-v"));
   if pasted && r.args.contains(&"capture-pane".into()) && !*self.checked.lock().unwrap(){let file=std::fs::OpenOptions::new().read(true).write(true).open(&self.path).unwrap();assert!(file.try_lock().is_err());*self.checked.lock().unwrap()=true;}
   self.inner.command(r).await
  })}
  fn socket<'a>(&'a self,d:&'a NativeMessage,e:Vec<u8>)->IoFuture<'a,WriteOutcome>{self.inner.socket(d,e)}
 }
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("clipboard.lock");let mut b=binding();b.windows=true;b.pane="=test:0.0".into();b.clipboard_lock_path=Some(path.clone());
 let io=Arc::new(Probe {inner:FakeIo::new(vec![screen(""),screen(""),screen("[Pasted text #1 +2 lines]"),screen("")]),path,checked:Mutex::new(false)});let s=Arc::new(Services::new());s.facts.lock().unwrap().binding=b.clone();
 let d=TerminalDriver::new(b,s,io.clone(),InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3});
 assert_eq!(d.prompt("ação\\😀\nsecond","id").await.disposition,Disposition::Accepted);assert!(*io.checked.lock().unwrap());
 let calls=io.inner.calls.lock().unwrap();let clip=calls.iter().find(|c|c.program=="powershell.exe").unwrap();assert_eq!(clip.stdin,"ação\\😀\nsecond".as_bytes());assert!(!calls.iter().any(|c|c.args.contains(&"load-buffer".into())));
}
#[cfg(unix)]
#[tokio::test]
async fn terminal_input_native_process_io_writes_envelope_and_missing_socket_is_safe() {
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("native.sock");let listener=tokio::net::UnixListener::bind(&path).unwrap();let native=NativeMessage {socket:path,origin:"uds:/inbox".into(),sender:"peer".into(),mode:"bypass".into(),message_id:Some("uuid-message".into())};
 let peer=tokio::spawn(async move{use tokio::io::AsyncReadExt;let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();socket.read_to_end(&mut bytes).await.unwrap();bytes});
 assert_eq!(ProcessIo::default().socket(&native,b"envelope\n".to_vec()).await.unwrap(),WriteOutcome::Written);assert_eq!(peer.await.unwrap(),b"envelope\n");
 // Socket ausente de fato: o ouvinte fechado ainda aceita conexão enquanto um `fork` de outro teste
 // da suíte segura o descritor dele até o `exec`.
 std::fs::remove_file(&native.socket).unwrap();
 assert_eq!(ProcessIo::default().socket(&native,b"envelope\n".to_vec()).await.unwrap(),WriteOutcome::NotWritten);
}
#[tokio::test]
async fn terminal_input_native_only_recognized_messages_and_safe_fallback() {
 for prefix in ["[de: peer] ",""] {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen(&format!("{prefix}hello")),screen("")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().native=Some(NativeMessage {socket:"/private/socket".into(),origin:"uds:/inbox".into(),sender:"peer".into(),mode:"bypass".into(),message_id:Some("uuid-message".into())});
 assert_eq!(driver(io.clone(),s).prompt(&format!("{prefix}hello"),"id").await.disposition,Disposition::Accepted);assert_eq!(io.writes().len(),2);
 }
}
#[tokio::test]
async fn terminal_input_partial_own_prefix_with_foreign_suffix_is_not_cleaned() {
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("long original message foreign suffix")]));*io.fail.lock().unwrap()=Some("long original message".into());
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await.disposition,Disposition::Unknown);assert_eq!(io.writes().len(),1);
}
#[tokio::test]
async fn terminal_input_old_picker_above_composer_does_not_block_prompt() {
 let past="Question\n❯ 1. first\n  2. second\nEsc to cancel · to navigate\nold answer\nold answer\nold answer\nold answer\n";
 let io=Arc::new(FakeIo::new(vec![format!("{past}{}",screen("")),screen("hello"),screen("")]));
 assert_eq!(driver(io,Arc::new(Services::new())).prompt("hello","id").await.disposition,Disposition::Accepted);
}
#[tokio::test]
async fn terminal_input_unnumbered_trust_cursor_is_real_option_position() {
 let first="Trust this folder?\n❯ Yes, trust\n  No, exit\nEnter to select";
 let second="Trust this folder?\n  Yes, trust\n❯ No, exit\nEnter to select";
 let io=Arc::new(FakeIo::new(vec![second.into(),first.into(),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).select(1,true).await.disposition,Disposition::Accepted);
 assert_eq!(io.writes()[0].args.last().unwrap(),"Up");
}
#[tokio::test]
async fn terminal_input_submit_selected_requires_final_proof() {
 let multi="Question\n❯ 1. [✔] first\n  2. [ ] second\nEsc to cancel · to navigate";
 let review="Review your answers\n→ first\n❯ 1. Submit answers\nEsc to cancel";
 let io=Arc::new(FakeIo::new(vec![multi.into(),review.into(),review.into()]));
 assert_eq!(driver(io,Arc::new(Services::new())).submit_selected().await.disposition,Disposition::Unknown);
}
#[tokio::test]
async fn terminal_input_guarded_submit_of_menu_without_sidecar_concludes_and_stale_menu_is_refused() {
 use hangar_server::interactions::{InteractionBinding, InteractionKind, InteractionStore, InteractionTarget};
 let multi="Question\n❯ 1. [✔] first\n  2. [ ] second\nEsc to cancel · to navigate";
 let other="Outra\n❯ 1. [ ] x\n  2. [ ] y\nEsc to cancel · to navigate";
 let review="Review your answers\n→ first\n❯ 1. Submit answers\nEsc to cancel";
 let tmp=tempfile::tempdir().unwrap();let jsonl=tmp.path().join("projects/p/uuid.jsonl");
 let menu=InteractionTarget {kind:InteractionKind::Menu,request_key:"terminal:menu".into()};
 // Identidade como o `observe_menu` publica: sem sidecar, sem revisão.
 let observed=|generation:u64,pane:&str| {
  let store=Arc::new(InteractionStore::default());let life=InteractionBinding {key:"k".into(),conversation:"uuid".into(),generation};
  store.bind(&life).unwrap();let id=store.observe(&life,&menu,Some(interaction_semantic(pane,None).unwrap())).unwrap().unwrap();
  let expected=store.expected(&life,Some(&id)).unwrap();(store,expected)
 };
 let run=|store:Arc<InteractionStore>,screens:Vec<String>| {let io=Arc::new(FakeIo::new(screens));(io.clone(),driver(io,Arc::new(Services::new())).with_interactions(store).with_interaction_source(jsonl.clone()))};
 let (store,expected)=observed(1,multi);
 // A conferência final de cada tecla relê a tela: o menu antes do `Right`, a aba de envio antes do Enter.
 let (io,d)=run(store,vec![multi.into(),multi.into(),multi.into(),review.into(),review.into(),screen("")]);
 let r=d.guarded_submit_selected(&expected).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"control_submitted"));
 assert_eq!(io.writes().iter().map(|w|w.args.last().unwrap().as_str()).collect::<Vec<_>>(),["Right","\r"]);
 // Menu que mudou e vida antiga continuam recusados, sem tecla nenhuma.
 for (generation,pane) in [(1,other),(2,multi)] {
  let (store,expected)=observed(generation,pane);
  let (io,d)=run(store,vec![multi.into()]);
  let r=d.guarded_submit_selected(&expected).await;
  assert_eq!((r.disposition,r.code.as_str()),(Disposition::Rejected,"erro_interacao_desatualizada"));assert!(io.writes().is_empty());
 }
 // Pergunta com sidecar não usa o envio do menu.
 let (store,mut expected)=observed(1,multi);expected.target.kind=InteractionKind::Question;expected.source_revision=Some("r".into());
 let (io,d)=run(store,vec![multi.into()]);
 let r=d.guarded_submit_selected(&expected).await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Rejected,"erro_interacao_indisponivel"));assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_answer_review_enter_requires_final_proof() {
 let review="Review your answers\n→ first\n❯ 1. Submit answers\nEsc to cancel";
 let io=Arc::new(FakeIo::new(vec![picker(),review.into(),review.into()]));
 assert_eq!(driver(io,Arc::new(Services::new())).answer(&[option_answer(0,"first")]).await.disposition,Disposition::Unknown);
}
#[tokio::test]
async fn terminal_input_clipboard_unavailable_preserves_literal_and_blocks_clipboard() {
 for text in ["hello","first\nsecond"] {let mut b=binding();b.windows=true;b.pane="=test:0.0".into();let dir=tempfile::tempdir().unwrap();b.clipboard_lock_path=Some(dir.path().join("clip.lock"));let s=Arc::new(Services::new());{let mut f=s.facts.lock().unwrap();f.binding=b.clone();f.clipboard_available=false;}
 let io=Arc::new(FakeIo::new(vec![screen(""),screen(text),screen("")]));let d=TerminalDriver::new(b,s,io.clone(),InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3});
 assert_eq!(d.prompt(text,"id").await.disposition,if text.contains('\n'){Disposition::Deferred}else{Disposition::Accepted});assert!(!io.calls.lock().unwrap().iter().any(|r|r.program=="powershell.exe"));}
}

#[tokio::test]
async fn terminal_input_windows_dash_placeholder_left_behind_fails_with_its_code() {
 // O `x` provisório que o pane não apagou mudaria o texto: falha com o código, nunca entregue.
 let mut b=binding();b.windows=true;b.pane="=test:0.0".into();
 let s=Arc::new(Services::new());s.facts.lock().unwrap().binding=b.clone();
 let io=Arc::new(FakeIo::new(vec![screen("x-abc")]));
 let r=TerminalDriver::new(b,s,io.clone(),instant_limits()).text("-abc").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Unknown,"placeholder_unremoved"));
 assert_eq!(keys(&io),["x-abc","Home","DC"]);
}
fn instant_limits() -> InputLimits { InputLimits { settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:3 } }
#[tokio::test]
async fn terminal_input_interleaved_interactive_pastes_keep_each_pane_bytes() {
 struct Interleaved {buffers:Mutex<std::collections::HashMap<String,Vec<u8>>>,received:Mutex<std::collections::HashMap<String,Vec<u8>>>,loaded:tokio::sync::Barrier}
 impl TerminalIo for Interleaved {
  fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput>{Box::pin(async move {
   let arg=|flag:&str|r.args.iter().position(|s|s==flag).map(|i|r.args[i+1].clone()).unwrap();
   if r.args.contains(&"display-message".into()){return Ok(CommandOutput {success:true,stdout:format!("test\t{}\t17\n",arg("-t")).into_bytes()});}
   if r.args.contains(&"load-buffer".into()){self.buffers.lock().unwrap().insert(arg("-b"),r.stdin);self.loaded.wait().await;return Ok(CommandOutput {success:true,stdout:vec![]});}
   if r.args.contains(&"paste-buffer".into()){let bytes=self.buffers.lock().unwrap().remove(&arg("-b"));let success=bytes.is_some();if let Some(bytes)=bytes{self.received.lock().unwrap().insert(arg("-t"),bytes);}return Ok(CommandOutput {success,stdout:vec![]});}
   panic!("unexpected command");
  })}
  fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>)->IoFuture<'a,WriteOutcome>{Box::pin(async{Ok(WriteOutcome::NotWritten)})}
 }
 let io=Arc::new(Interleaved {buffers:Mutex::new(std::collections::HashMap::new()),received:Mutex::new(std::collections::HashMap::new()),loaded:tokio::sync::Barrier::new(2)});
 let a=TerminalDriver::new(binding(),Arc::new(Services::new()),io.clone(),instant_limits());
 let mut b_binding=binding();b_binding.pane="%8".into();let b_services=Arc::new(Services::new());b_services.facts.lock().unwrap().binding=b_binding.clone();
 let b=TerminalDriver::new(b_binding,b_services,io.clone(),instant_limits());
 let (a_result,b_result)=tokio::join!(a.text("first A\nsecond A"),b.text("first B\nsecond B"));
 assert_eq!(a_result.disposition,Disposition::Accepted);assert_eq!(b_result.disposition,Disposition::Accepted);
 let received=io.received.lock().unwrap();assert_eq!(received.get("%7").unwrap(),b"first A\nsecond A");assert_eq!(received.get("%8").unwrap(),b"first B\nsecond B");
}
#[tokio::test]
async fn terminal_input_plugin_no_write_rechecks_all_guards_without_erasing_new_draft() {
 struct Gate {inner:Services,entered:tokio::sync::Notify,resume:tokio::sync::Notify}
 impl TerminalServices for Gate {
  fn facts<'a>(&'a self,b:&'a TerminalBinding)->ServiceFuture<'a,InputFacts>{self.inner.facts(b)}
  fn publish<'a>(&'a self,b:&'a TerminalBinding,r:PluginRequest)->ServiceFuture<'a,PluginReply>{Box::pin(async move{self.entered.notify_one();self.resume.notified().await;self.inner.publish(b,r).await})}
  fn permission_guard<'a>(&'a self,b:&'a TerminalBinding)->ServiceFuture<'a,()>{self.inner.permission_guard(b)}
 }
 for reply in [PluginReply::Unavailable,PluginReply::NotWritten] {for change in ["question","ready","overlay","draft"] {
  let s=Arc::new(Gate {inner:Services::new(),entered:tokio::sync::Notify::new(),resume:tokio::sync::Notify::new()});s.inner.facts.lock().unwrap().plugin_live=true;*s.inner.reply.lock().unwrap()=reply.clone();
  let io=Arc::new(FakeIo::new(vec![screen("")]));let d=TerminalDriver::new(binding(),s.clone(),io.clone(),instant_limits());let task=tokio::spawn(async move{d.prompt("message","id").await});s.entered.notified().await;
  match change {"question"=>s.inner.facts.lock().unwrap().open_question=true,"ready"=>s.inner.facts.lock().unwrap().ready=false,"overlay"=>*io.screens.lock().unwrap()=vec![picker()].into(),"draft"=>*io.screens.lock().unwrap()=vec![screen("new owner draft")].into(),_=>unreachable!()}
  s.resume.notify_one();let r=task.await.unwrap();assert_eq!(r.disposition,Disposition::Deferred,"{reply:?}/{change}");assert!(io.writes().is_empty(),"{reply:?}/{change}");assert_eq!(s.inner.published.lock().unwrap().len(),1);
 }}
}
#[tokio::test]
async fn terminal_input_clipboard_wait_rechecks_all_guards_without_erasing_new_draft() {
 for change in ["question","ready","overlay","draft"] {
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("clipboard.lock");let lock=std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).unwrap();lock.lock().unwrap();
  let mut b=binding();b.windows=true;b.pane="=test:0.0".into();b.clipboard_lock_path=Some(path);let s=Arc::new(Services::new());s.facts.lock().unwrap().binding=b.clone();let io=Arc::new(FakeIo::new(vec![screen("")]));let mut limits=instant_limits();limits.settle=Duration::from_millis(1);limits.ready_attempts=100;
  let d=TerminalDriver::new(b,s.clone(),io.clone(),limits);let task=tokio::spawn(async move{d.prompt("message\nsecond","id").await});while !io.calls.lock().unwrap().iter().any(|c|c.args.contains(&"capture-pane".into())){tokio::task::yield_now().await;}
  match change {"question"=>s.facts.lock().unwrap().open_question=true,"ready"=>s.facts.lock().unwrap().ready=false,"overlay"=>*io.screens.lock().unwrap()=vec![picker()].into(),"draft"=>*io.screens.lock().unwrap()=vec![screen("new owner draft")].into(),_=>unreachable!()}
  drop(lock);let r=task.await.unwrap();assert_eq!(r.disposition,Disposition::Deferred,"{change}");assert!(io.writes().is_empty(),"{change}");assert!(!io.calls.lock().unwrap().iter().any(|r|r.program=="powershell.exe"),"{change}");
 }
}
#[tokio::test]
async fn terminal_input_answer_first_arrow_uncertain_never_defers_or_submits() {
 let io=Arc::new(FakeIo::new(vec![picker()]));*io.fail.lock().unwrap()=Some("Down".into());let r=driver(io.clone(),Arc::new(Services::new())).answer(&[option_answer(1,"second")]).await;
 // Só a seta pode ter saído, sem Enter: recusa com o código (nada respondido), como no `/select`.
 assert_eq!(r.disposition,Disposition::Rejected);assert_eq!(io.writes().len(),1);assert_eq!(io.writes()[0].args.last().unwrap(),"Down");
}
#[tokio::test]
async fn terminal_input_answer_arrow_accepted_then_capture_failed_is_refused_without_enter() {
 struct CaptureFailure(FakeIo);
 impl TerminalIo for CaptureFailure {
  fn command<'a>(&'a self,r:CommandRequest)->IoFuture<'a,CommandOutput>{Box::pin(async move{let navigated=!self.0.writes().is_empty();if navigated && r.args.contains(&"capture-pane".into()){return Err(IoFailure {code:"capture_failed",may_have_written:false});}self.0.command(r).await})}
  fn socket<'a>(&'a self,d:&'a NativeMessage,e:Vec<u8>)->IoFuture<'a,WriteOutcome>{self.0.socket(d,e)}
 }
 let io=Arc::new(CaptureFailure(FakeIo::new(vec![picker()])));let d=TerminalDriver::new(binding(),Arc::new(Services::new()),io.clone(),instant_limits());let r=d.answer(&[option_answer(1,"second")]).await;
 assert_eq!(r.disposition,Disposition::Rejected);assert_eq!(io.0.writes().len(),1);assert_eq!(io.0.writes()[0].args.last().unwrap(),"Down");
}

#[test]
fn terminal_answer_preserves_public_question_identity() {
    let answer: QuestionAnswer = serde_json::from_value(serde_json::json!({
        "kind":"text", "indices":[], "labels":[], "question_id":"question-a",
        "value":"texto", "type_index":1, "chat_index":null, "multi":false
    })).unwrap();
    assert_eq!(answer.question_id.as_deref(), Some("question-a"));
}
fn suggestion_screen(s: &str) -> String {
 let rule = format!("\u{1b}[38;2;136;136;136m{}\u{1b}[39m", "─".repeat(30));
 let typed = if s.is_empty() { "\u{1b}[2mTry \"refactor <filepath>\"\u{1b}[0m".to_string() } else { s.to_string() };
 format!("history\n{rule}\n\u{1b}[39m❯ {typed}\n{rule}\n⏵⏵ bypass permissions\n\n")
}
#[test]
fn terminal_input_unstyle_drops_only_dim_text() {
 assert_eq!(unstyle("\u{1b}[38;2;1;2;3m──\u{1b}[39m a \u{1b}[2mghost\u{1b}[0m b\n\u{1b}[38;5;2mc\u{1b}[m", false), "── a  b\nc");
 assert_eq!(unstyle("x \u{1b}[2mghost\u{1b}[22m y", true), "x ghost y");
 assert!(ComposerSnapshot::parse(&unstyle(&suggestion_screen(""), false)).unwrap().is_empty());
 assert!(!ComposerSnapshot::parse(&unstyle(&suggestion_screen("[Pasted text #1 +3 lines]"), false)).unwrap().is_empty());
}
#[tokio::test]
async fn terminal_input_dim_suggestion_is_an_empty_composer_before_and_after_submit() {
 let io=Arc::new(FakeIo::new(vec![suggestion_screen(""),suggestion_screen("ok"),suggestion_screen("")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("ok","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"submitted"));
 let writes=io.writes(); assert_eq!(writes.len(),2); assert!(writes.iter().all(|w|w.args.last().unwrap()!="C-u"));
}
struct ClearIo { inner: FakeIo, services: Arc<Services> }
impl TerminalIo for ClearIo {
 fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
  // O Enter do `/clear` troca a conversa: a partir daí os fatos não batem mais com o vínculo.
  if r.args.last().is_some_and(|a| a == "\r") { self.services.facts.lock().unwrap().binding.conversation = "after-clear".into(); }
  self.inner.command(r).await }) }
 fn socket<'a>(&'a self, n: &'a NativeMessage, e: Vec<u8>) -> IoFuture<'a, WriteOutcome> { self.inner.socket(n, e) }
}
#[tokio::test]
async fn terminal_input_clear_is_proved_after_its_own_conversation_change() {
 let s=Arc::new(Services::new());
 let io=Arc::new(ClearIo { inner: FakeIo::new(vec![screen(""),screen("/clear"),screen("")]), services: s.clone() });
 let d=TerminalDriver::new(binding(),s.clone(),io.clone(),InputLimits {literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2});
 let r=d.prompt("/clear","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"submitted"));
 // Outro texto depois da troca continua exigindo o vínculo.
 assert_ne!(d.prompt("hello","id2").await.disposition,Disposition::Accepted);
}
#[tokio::test]
async fn terminal_input_sent_text_left_dim_in_composer_is_not_proof_of_submit() {
 let rule = format!("\u{1b}[38;2;136;136;136m{}\u{1b}[39m", "─".repeat(30));
 let dim_ok = format!("history\n{rule}\n\u{1b}[39m❯ \u{1b}[2mok\u{1b}[0m\n{rule}\n⏵⏵ bypass permissions\n\n");
 let io=Arc::new(FakeIo::new(vec![suggestion_screen(""),suggestion_screen("ok"),dim_ok]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("ok","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Unknown,"submit_unproved"));
}
/// Tela do psmux com `-e`: a sugestão vem esmaecida (`0;2m`) depois de um NBSP.
fn psmux_screen(typed: &str) -> String {
 let rule = format!("\u{1b}[0;38;2;136;136;136m{}\u{1b}[0m", "─".repeat(30));
 let typed = if typed.is_empty() { "\u{1b}[0;2mTry \"fix typecheck errors\"\u{1b}[0m".to_string() } else { typed.to_string() };
 format!("\u{1b}[0mhistory\u{1b}[0m\n{rule}\n\u{1b}[0m❯\u{a0}{typed}\n{rule}\n\u{1b}[0m  \u{1b}[0;38;2;255;193;7m⏵⏵ auto mode on\u{1b}[0m\n")
}
/// Como o psmux: sem `-e` a captura sai sem estilo, e a sugestão vira texto comum.
struct PsmuxIo(FakeIo);
impl TerminalIo for PsmuxIo {
 fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
  let styled = r.args.iter().any(|a| a == "-e");
  let mut out = self.0.command(r.clone()).await?;
  if r.args.iter().any(|a| a == "capture-pane") && !styled { out.stdout = unstyle(&String::from_utf8(out.stdout).unwrap(), true).into_bytes(); }
  Ok(out) }) }
 fn socket<'a>(&'a self, n: &'a NativeMessage, e: Vec<u8>) -> IoFuture<'a, WriteOutcome> { self.0.socket(n, e) }
}
#[tokio::test]
async fn terminal_input_windows_dim_suggestion_is_an_empty_composer() {
 let mut b=binding();b.windows=true;b.pane="=test:0.0".into();
 let s=Arc::new(Services::new());s.facts.lock().unwrap().binding=b.clone();
 let io=Arc::new(PsmuxIo(FakeIo::new(vec![psmux_screen(""),psmux_screen("ola"),psmux_screen("")])));
 let r=TerminalDriver::new(b,s,io.clone(),instant_limits()).prompt("ola","id").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"submitted"));
 let writes=io.0.writes(); assert!(writes.iter().all(|w|w.args.last().unwrap()!="C-u"));
}
fn stashed(s: &str) -> String { screen(s).replacen("history\n","history\n  ctrl+g to edit · › stashed\n",1) }
fn keys(io:&FakeIo)->Vec<String> { io.writes().iter().map(|w|w.args.last().unwrap().clone()).collect() }
#[tokio::test]
async fn terminal_input_owner_stash_in_use_is_never_overwritten() {
 let io=Arc::new(FakeIo::new(vec![stashed("rascunho novo")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("mensagem","id").await;
 assert_eq!(r.disposition,Disposition::Deferred);assert_eq!(r.code,"composer_busy");assert!(io.writes().is_empty());assert_eq!(r.draft,None);
}
#[tokio::test]
async fn terminal_input_draft_given_back_when_nothing_was_sent() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho"),stashed(""),stashed("mensagem"),stashed(""),stashed(""),screen("rascunho")]));*io.fail.lock().unwrap()=Some("mensagem".into());
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("mensagem","id").await;
 assert_eq!(r.disposition,Disposition::Deferred);assert_eq!(r.cleanup,Cleanup::Proved);assert_eq!(r.draft,Some(DraftOutcome::Returned));
 assert_eq!(keys(&io),["C-s","mensagem","C-u","C-s"]);
}
#[tokio::test]
async fn terminal_input_uncertain_submit_never_unstashes_blindly() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho"),stashed(""),stashed("long original message")]));*io.fail.lock().unwrap()=Some("\r".into());
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await;
 assert_eq!(r.disposition,Disposition::Unknown);assert_eq!(r.draft,Some(DraftOutcome::Stashed));
 assert_eq!(keys(&io),["C-s","long original message","\r"]);
}
#[tokio::test]
async fn terminal_input_submit_proved_by_the_stash_coming_back() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho [Pasted text #1 +3 lines]"),stashed(""),stashed("long original message"),screen("rascunho [Pasted text #1 +3 lines]")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("long original message","id").await;
 assert_eq!(r.disposition,Disposition::Accepted,"{}",r.code);assert_eq!(r.draft,Some(DraftOutcome::Returned));
 assert_eq!(keys(&io),["C-s","long original message","\r"]);
}
#[tokio::test]
async fn terminal_input_new_owner_text_is_never_stashed_over_the_draft() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho"),stashed(""),stashed("novo do dono")]));let s=Arc::new(Services::new());s.facts.lock().unwrap().plugin_live=true;
 let r=driver(io.clone(),s).prompt("mensagem","id").await;
 assert_eq!(r.disposition,Disposition::Deferred);assert_eq!(r.code,"composer_busy");assert_eq!(r.draft,Some(DraftOutcome::Stashed));
 assert_eq!(keys(&io),["C-s"]);
}
#[tokio::test]
async fn terminal_input_cli_without_stash_keeps_the_draft_and_waits() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("mensagem","id").await;
 assert_eq!(r.disposition,Disposition::Deferred);assert_eq!(r.code,"composer_busy");assert_eq!(r.draft,Some(DraftOutcome::Returned));
 assert_eq!(keys(&io),["C-s"]);
}
#[tokio::test]
async fn terminal_input_stash_hint_gone_with_our_paste_left_is_not_a_submit() {
 let io=Arc::new(FakeIo::new(vec![screen("rascunho"),stashed(""),stashed("[Pasted text #1 +2 lines]"),screen("[Pasted text #1 +2 lines]")]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("first\nsecond","id").await;
 assert_eq!(r.disposition,Disposition::Unknown);assert_eq!(r.draft,Some(DraftOutcome::Unverified));
}
#[test]
fn pane_formats_never_read_empty_as_zero() {
 assert_eq!(PaneFormats::parse("1|1|0|150|45\n"), Some(PaneFormats { mouse: true, in_mode: false, columns: 150, rows: 45 }));
 assert_eq!(PaneFormats::parse("|1|0|150|45"), Some(PaneFormats { mouse: true, in_mode: false, columns: 150, rows: 45 }), "psmux: sem mouse_sgr_flag vale a tela alternativa");
 assert!(!PaneFormats::parse("0|1|0|150|45").unwrap().mouse);
 assert_eq!(PaneFormats::parse("1|1||150|45"), None, "pane_in_mode vazio não é fora de modo");
 assert_eq!(PaneFormats::parse("1|1|0|x|45"), None);
}
#[tokio::test]
async fn mouse_wheel_keys_and_resize_go_as_measured() {
 let io = Arc::new(FakeIo::new(vec![screen("")]));
 let d = driver(io.clone(), Arc::new(Services::new()));
 d.mouse(0, 104).await.unwrap();
 d.wheel(19, 114, true).await.unwrap();
 d.mods_keys(&["C-x", "Tab"]).await.unwrap();
 d.mods_keys(&["Enter"]).await.unwrap();
 assert_eq!(d.mods_keys(&["Escape"]).await.unwrap_err().code, "key_not_allowed");
 d.resize(144, 45).await.unwrap();
 let writes: Vec<Vec<String>> = io.calls.lock().unwrap().iter().filter(|r| !r.args.iter().any(|a| a == "display-message")).map(|r| r.args[2..].to_vec()).collect();
 let expected: Vec<Vec<String>> = [
  vec!["send-keys", "-t", "%7", "-l", "--", "\u{1b}[<0;105;1M\u{1b}[<0;105;1m"],
  vec!["send-keys", "-t", "%7", "-l", "--", "\u{1b}[<65;115;20M"],
  vec!["send-keys", "-t", "%7", "C-x", "Tab"],
  vec!["send-keys", "-t", "%7", "-l", "--", "\r"],
  vec!["resize-window", "-t", "=test:", "-x", "144", "-y", "45"],
  vec!["set-window-option", "-t", "=test:", "window-size", "latest"],
 ].into_iter().map(|v| v.into_iter().map(String::from).collect()).collect();
 assert_eq!(writes, expected);
 assert!(io.calls.lock().unwrap().iter().filter(|r| r.args.iter().any(|a| a == "display-message")).count() >= 5, "cada efeito confere o pane antes");
}
#[tokio::test]
async fn windows_counts_terminals_by_session_attached() {
 // No psmux o Hangar não abre cliente de controle: o observador da prévia recusa no Windows
 // (`terminal_control.rs:173`) e o vigia de tamanho também (`watch_notices`, Task 15, com o teste
 // `the_watch_refuses_on_windows`). Só por isso `#{session_attached}` conta terminais de verdade lá; o
 // `list-clients -F` do psmux é medido na Task 20.
 let io = Arc::new(FakeIo::new(vec![screen("")]));
 let mut b = binding(); b.windows = true; b.pane = "=test:0.0".into();
 let s = Arc::new(Services::new()); s.facts.lock().unwrap().binding = b.clone();
 let d = TerminalDriver::new(b, s, io.clone(), InputLimits::default());
 *io.attached.lock().unwrap() = "1\n".into();
 assert_eq!(d.mods_clients().await.unwrap(), 1);
 *io.attached.lock().unwrap() = "0\n".into();
 assert_eq!(d.mods_clients().await.unwrap(), 0);
 *io.attached.lock().unwrap() = "\n".into();
 assert_eq!(d.mods_clients().await.unwrap_err().code, "clients_unreadable", "vazio nunca vale 0");
 assert!(!io.calls.lock().unwrap().iter().any(|r| r.args.iter().any(|a| a == "list-clients")));
 *io.formats.lock().unwrap() = "|1|0|150|45\n".into();
 assert!(d.mods_formats().await.unwrap().mouse, "psmux: sem mouse_sgr_flag vale a tela alternativa");
}
#[tokio::test]
async fn clients_skip_control_mode() {
 let io = Arc::new(FakeIo::new(vec![screen("")]));
 *io.clients.lock().unwrap() = "attached,focused,control-mode,ignore-size,no-output,UTF-8\nattached,focused,UTF-8\n".into();
 let d = driver(io.clone(), Arc::new(Services::new()));
 assert_eq!(d.mods_clients().await.unwrap(), 1);
 *io.clients.lock().unwrap() = "attached,focused,control-mode,ignore-size,no-output,UTF-8\n".into();
 assert_eq!(d.mods_clients().await.unwrap(), 0, "o observador e o vigia do Hangar não são terminal ligado");
 assert_eq!(d.mods_formats().await.unwrap(), PaneFormats { mouse: true, in_mode: false, columns: 150, rows: 45 });
 let capture = { d.mods_screen().await.unwrap(); io.calls.lock().unwrap().iter().rev().find(|r| r.args.iter().any(|a| a == "capture-pane")).unwrap().args.clone() };
 assert!(capture.contains(&"-e".to_string()) && !capture.contains(&"-S".to_string()), "só a parte visível, com atributos");
}
// #85: com o foco no painel de agentes o texto digitado some (e o `x` dele para um subagente).
const AGENTS_FOCUSED: &str = include_str!("../../../../backend/tests/fixtures/pane_agents_panel_focused.txt");
const AGENTS_FOOTER: &str = include_str!("../../../../backend/tests/fixtures/pane_agents_footer_focused.txt");
#[tokio::test]
async fn terminal_input_footer_focus_returns_to_composer_with_escape_before_typing() {
 for focused in [AGENTS_FOCUSED, AGENTS_FOOTER] {
  let io=Arc::new(FakeIo::new(vec![focused.into(),screen(""),screen("hello"),screen("")]));
  assert_eq!(driver(io.clone(),Arc::new(Services::new())).prompt("hello","id").await.disposition,Disposition::Accepted);
  let writes=io.writes(); assert_eq!(writes[0].args.last().unwrap(),"Escape"); assert_eq!(writes[1].args.last().unwrap(),"hello");
 }
}
#[tokio::test]
async fn terminal_input_footer_focus_that_stays_defers_without_typing() {
 let io=Arc::new(FakeIo::new(vec![AGENTS_FOCUSED.into()]));
 let r=driver(io.clone(),Arc::new(Services::new())).prompt("/clear","id").await;
 assert_eq!(r.disposition,Disposition::Deferred); assert_eq!(r.code,"footer_focus");
 let writes=io.writes(); assert_eq!(writes.len(),1); assert_eq!(writes[0].args.last().unwrap(),"Escape");
}
#[test]
fn terminal_state_agents_panel_is_not_a_menu() {
 for pane in [AGENTS_FOCUSED, include_str!("../../../../backend/tests/fixtures/pane_agents_panel_focused_working.txt")] {
  let analysis=hangar_server::terminal_state::analyze(pane);
  assert_ne!(analysis.state,"awaiting_input"); assert!(analysis.options.is_none());
  assert!(hangar_server::terminal_state::footer_focus(pane));
 }
 assert!(!hangar_server::terminal_state::footer_focus(&screen("")));
 assert!(!hangar_server::terminal_state::footer_focus(include_str!("../../../../backend/tests/fixtures/pane_trust_dialog.txt")));
}
#[tokio::test]
async fn terminal_input_interrupt_with_footer_focus_returns_focus_first() {
 // O primeiro Esc só devolve o foco do painel de agentes ao composer: o segundo interrompe.
 let io=Arc::new(FakeIo::new(vec![AGENTS_FOCUSED.into(),screen("")]));
 assert_eq!(driver(io.clone(),Arc::new(Services::new())).interrupt(false).await.disposition,Disposition::Accepted);
 let escapes=io.writes().iter().filter(|r|r.args.last().unwrap()=="Escape").count(); assert_eq!(escapes,2);
 let io=Arc::new(FakeIo::new(vec![screen("")]));
 driver(io.clone(),Arc::new(Services::new())).interrupt(false).await;
 assert_eq!(io.writes().iter().filter(|r|r.args.last().unwrap()=="Escape").count(),1);
}
// `claude --name` (ou `/rename`) escreve o nome na régua de cima da caixa de digitar: a leitura não muda.
#[test]
fn terminal_state_session_name_in_the_rule_reads_the_same() {
 use hangar_server::terminal_state::{analyze, footer_focus, is_rule};
 let named = |pane: &str| {
  let lines: Vec<&str> = pane.lines().collect();
  let top = (0..lines.len() - 1).rev().find(|&i| is_rule(lines[i]) && lines[i + 1].trim_start().starts_with('❯')).unwrap();
  let width = lines[top].chars().count();
  let label = format!("{} minha-sessao ─", "─".repeat(width - " minha-sessao ─".chars().count()));
  lines.iter().enumerate().map(|(i, l)| if i == top { label.as_str() } else { l }).collect::<Vec<_>>().join("\n")
 };
 let mut previews = 0;
 for pane in [AGENTS_FOCUSED, AGENTS_FOOTER,
  include_str!("../../../../backend/tests/fixtures/pane_agents_panel_focused_working.txt"),
  include_str!("../../../../backend/tests/fixtures/pane_idle.txt"),
  include_str!("../../../../backend/tests/fixtures/pane_thinking.txt"),
  include_str!("../../../../backend/tests/fixtures/pane_ferramenta_em_voo_acesa.txt"),
  include_str!("../../../../backend/tests/fixtures/pane_ferramenta_em_voo_apagada.txt")] {
  let renamed = named(pane);
  assert_ne!(renamed, pane.trim_end_matches('\n'));
  assert_eq!(analyze(&renamed), analyze(pane));
  assert_eq!(footer_focus(&renamed), footer_focus(pane));
  previews += usize::from(!analyze(pane).preview.is_empty());
 }
 assert!(previews > 0, "alguma captura precisa ter prévia em voo");
}

// ── Seletor do `/model`: o golden do driver Python (`gen_model_picker.py`) ──────────────────────────
fn picker_case(name: &str) -> serde_json::Value {
 let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/tests/fixtures/contract/golden/model_picker.json");
 let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
 data["cases"].as_array().unwrap().iter().find(|case| case["name"] == name).unwrap_or_else(|| panic!("caso {name} fora do golden")).clone()
}
/// As teclas no nome do Python: o texto literal como veio, o `\r` como `Enter`.
fn picker_keys(io: &FakeIo) -> Vec<String> {
 io.writes().iter().map(|r| { let last = r.args.last().unwrap().clone(); if last == "\r" { "Enter".into() } else { last } }).collect()
}
fn picker_screens(case: &serde_json::Value) -> Vec<String> { case["panes"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_owned()).collect() }
/// Roda um caso do golden: as capturas do Python em sequência, as teclas e a resposta comparadas.
async fn picker_replay(name: &str) {
 let case = picker_case(name);
 let io = Arc::new(FakeIo::new(picker_screens(&case)));
 let request = &case["request"];
 // O que a rota entrega ao driver; a validação em si tem teste próprio.
 let raw = (request["model"].as_str().map(str::to_owned), request["effort"].as_str().map(str::to_owned));
 let (model, effort) = hangar_server::model_picker::normalize(request["model"].as_str(), request["effort"].as_str()).unwrap_or(raw);
 let out = hangar_server::model_picker::drive(&driver(io.clone(), Arc::new(Services::new())), model.as_deref(), effort.as_deref(), request["scope"] == "default").await;
 let want: Vec<String> = case["keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_owned()).collect();
 assert_eq!(picker_keys(&io), want, "{name}: teclas");
 match (&case["expect"]["ok"], out) {
  (serde_json::Value::Null, Err(refusal)) => assert_eq!(refusal.detail, case["expect"]["error"]["detail"].as_str().unwrap(), "{name}"),
  (serde_json::Value::Null, Ok(value)) => panic!("{name}: esperava recusa, veio {value}"),
  (want, Ok(value)) => assert_eq!(&value, want, "{name}"),
  (want, Err(refusal)) => panic!("{name}: esperava {want}, veio {refusal:?}"),
 }
}
/// B12: menu aberto e spinner animando recusam sem tecla; marcador de turno parado não.
#[tokio::test]
async fn model_picker_b12_refuses_menu_and_working_session() {
 for name in ["overlay_refuses", "spinner_live_refuses", "spinner_frozen_goes"] { picker_replay(name).await; }
}
/// B13: `/model`, Enter, espera o rodapé; 2º Enter só sem seletor; não abriu = Esc + 409.
#[tokio::test]
async fn model_picker_b13_opens_waits_and_aborts() {
 for name in ["half_redraw_then_open", "partial_then_drawn"] { picker_replay(name).await; }
}
/// Seletor que não abriu: as teclas do golden, e a recusa com código e frase em português (o Python a dava
/// em inglês e sem código).
#[tokio::test]
async fn model_picker_that_never_opens_is_refused_with_a_code() {
 let case = picker_case("never_opens");
 let io = Arc::new(FakeIo::new(picker_screens(&case)));
 let out = hangar_server::model_picker::drive(&driver(io.clone(), Arc::new(Services::new())), Some("sonnet"), None, false).await;
 let want: Vec<String> = case["keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_owned()).collect();
 assert_eq!(picker_keys(&io), want);
 let refusal = out.unwrap_err();
 assert_eq!((refusal.code, refusal.detail.as_str()), (Some("erro_seletor_modelo_nao_abriu"), "o seletor do /model não abriu no terminal da sessão"));
}
/// B14: navega pelo id antes da palavra-chave, só com setas; modelo ausente = Esc + 409.
#[tokio::test]
async fn model_picker_b14_navigates_by_id_with_arrows() {
 for name in ["session_navigates_and_presses_s", "variant_1m", "absent_model"] { picker_replay(name).await; }
}
/// B15: Right até o alvo acima, relê uma vez; sem suporte, nenhuma tecla. O `effort_cycle_unavailable` do
/// golden prende a volta do slider que o Python esperava e o Claude não dá: o caso novo está abaixo.
#[tokio::test]
async fn model_picker_b15_effort_moves_toward_the_target() {
 for name in ["effort_with_right", "effort_rereads_once", "effort_not_supported", "model_and_effort"] { picker_replay(name).await; }
}
/// O slider do esforço com o marcador em `level`, a partir de uma captura real do golden.
fn effort_pane(level: &str) -> String {
 picker_screens(&picker_case("effort_with_right"))[1].replace("xHigh effort", &format!("{level} effort"))
}
async fn effort_drive(screens: Vec<String>, target: &str) -> (Vec<String>, Result<serde_json::Value, hangar_server::model_picker::Refusal>) {
 let io = Arc::new(FakeIo::new(screens));
 let out = hangar_server::model_picker::drive(&driver(io.clone(), Arc::new(Services::new())), None, Some(target), false).await;
 (picker_keys(&io), out)
}
/// Alvo abaixo anda com Left (o slider não dá a volta: Right a partir de High iria a Max).
#[tokio::test]
async fn model_picker_effort_below_moves_left() {
 let case = picker_screens(&picker_case("effort_with_right"));
 let (keys, out) = effort_drive(vec![case[0].clone(), effort_pane("High"), effort_pane("Medium"), case[3].clone()], "medium").await;
 assert_eq!(keys, ["/model", "Enter", "Left", "s"]);
 assert!(out.is_ok(), "{out:?}");
}
/// O redesenho que atrasa um passo (marcador ainda no anterior) relê uma vez antes de dizer que parou.
#[tokio::test]
async fn model_picker_effort_rereads_a_late_redraw_before_refusing() {
 let case = picker_screens(&picker_case("effort_with_right"));
 let (keys, out) = effort_drive(vec![case[0].clone(), effort_pane("High"), effort_pane("High"), effort_pane("Medium"), case[3].clone()], "medium").await;
 assert!(out.is_ok(), "{out:?}");
 assert_eq!(keys, ["/model", "Enter", "Left", "s"]);
}
/// Marcador que não muda (Right no Max) ou que pula o alvo é recusa com código, com Esc e sem confirmar.
#[tokio::test]
async fn model_picker_effort_that_cannot_reach_the_target_is_refused() {
 let idle = picker_screens(&picker_case("effort_with_right"))[0].clone();
 for (start, after, target) in [("Max", "Max", "ultracode"), ("High", "Max", "xhigh"), ("Medium", "Medium", "low")] {
  let (keys, out) = effort_drive(vec![idle.clone(), effort_pane(start), effort_pane(after)], target).await;
  assert_eq!(out.unwrap_err().code, Some("erro_esforco_indisponivel"), "{start}→{target}");
  assert_eq!(keys.iter().filter(|k| *k == "s").count(), 0, "{start}→{target}: não confirma");
  assert!(keys.last().is_some_and(|k| k.contains('\u{1b}') || k == "Escape"), "{start}→{target}: fecha com Esc: {keys:?}");
 }
}
/// B16: `s` na sessão, Enter no padrão (o modelo só de espaço também confirma).
#[tokio::test]
async fn model_picker_b16_confirms_by_scope() {
 for name in ["default_presses_enter", "blank_model_presses_s", "model_normalized"] { picker_replay(name).await; }
}
/// B17: diálogo "Change effort level?" responde `pending_confirm`, sem tecla a mais.
#[tokio::test]
async fn model_picker_b17_pending_confirm() { picker_replay("pending_confirm").await; }
/// B18: seletor ainda aberto depois de confirmar = Esc + 409.
#[tokio::test]
async fn model_picker_b18_still_open_escapes() { picker_replay("still_open_after_confirm").await; }
/// B19: pediu a sessão e a linha diz padrão = 409, sem Esc.
#[tokio::test]
async fn model_picker_b19_session_switch_that_saved_default() { picker_replay("session_got_default").await; }
/// B22: no psmux, as mesmas teclas pelo driver comum (Enter como tecla, Esc na sequência do psmux).
#[tokio::test]
async fn model_picker_b22_psmux_same_sequence() {
 let io = Arc::new(FakeIo::new(picker_screens(&picker_case("never_opens"))));
 let mut b = binding(); b.windows = true; b.pane = "=test:0.0".into();
 let services = Arc::new(Services::new()); services.facts.lock().unwrap().binding = b.clone();
 let driver = TerminalDriver::new(b, services, io.clone(), InputLimits { literal_settle: Duration::ZERO, multiline_settle: Duration::ZERO, slash_settle: Duration::ZERO, settle: Duration::ZERO, proof_attempts: 2, ready_attempts: 2, cleanup_attempts: 3 });
 let out = hangar_server::model_picker::drive(&driver, Some("sonnet"), None, false).await;
 assert_eq!(picker_keys(&io), ["/model", "Enter", "Enter", "\u{1b}[27;1;27;1;0;1_\u{1b}[27;1;27;0;0;1_"]);
 assert_eq!(out.unwrap_err().code, Some("erro_seletor_modelo_nao_abriu"));
}
/// Os hooks `UserPromptSubmit` da conta começaram para este texto e ainda não terminaram: o plugin não
/// responde e o composer segue com o texto. O aviso do hook já basta para dar a entrada por aceita.
struct HooksStarted { inner: Services, started: String, abandoned: Mutex<Vec<String>> }
impl TerminalServices for HooksStarted {
 fn facts<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> { self.inner.facts(b) }
 fn publish<'a>(&'a self, b: &'a TerminalBinding, r: PluginRequest) -> ServiceFuture<'a, PluginReply> { Box::pin(async move { let _ = self.inner.publish(b, r).await; std::future::pending().await }) }
 fn permission_guard<'a>(&'a self, b: &'a TerminalBinding) -> ServiceFuture<'a, ()> { self.inner.permission_guard(b) }
 fn abandon_publication<'a>(&'a self, id: &'a str) -> ServiceFuture<'a, ()> { Box::pin(async move { self.abandoned.lock().unwrap().push(id.into()); Ok(()) }) }
 fn prompt_started<'a>(&'a self, _since: f64, text: &'a str) -> ServiceFuture<'a, bool> { Box::pin(async move { Ok(text == self.started) }) }
}
#[tokio::test]
async fn terminal_input_plugin_user_accepts_when_the_prompt_hooks_start() {
 let io=Arc::new(FakeIo::new(vec![screen("")]));
 let s=Arc::new(HooksStarted { inner: Services::new(), started: "hello".into(), abandoned: Mutex::new(vec![]) });
 s.inner.facts.lock().unwrap().plugin_live=true;s.inner.facts.lock().unwrap().plugin_user=true;
 let r=tokio::time::timeout(Duration::from_secs(5),driver(io.clone(),s.clone()).prompt("hello","op-1")).await.expect("não esperou o plugin");
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"prompt_started"));
 assert_eq!(*s.abandoned.lock().unwrap(),vec!["op-1".to_string()]);
 assert!(io.writes().is_empty());
}
#[tokio::test]
async fn terminal_input_slash_accepts_when_the_prompt_hooks_start_without_a_second_enter() {
 // O composer segue mostrando o comando enquanto os hooks rodam: sem o aviso, isso pediria outro Enter.
 let io=Arc::new(FakeIo::new(vec![screen(""),screen("/pr-review-auto 225")]));
 let s=Arc::new(HooksStarted { inner: Services::new(), started: "/pr-review-auto 225".into(), abandoned: Mutex::new(vec![]) });
 let r=driver(io.clone(),s).prompt("/pr-review-auto 225","op-2").await;
 assert_eq!((r.disposition,r.code.as_str()),(Disposition::Accepted,"prompt_started"));
 assert_eq!(io.writes().iter().filter(|w|w.args.last().is_some_and(|a|a=="\r")).count(),1);
}
