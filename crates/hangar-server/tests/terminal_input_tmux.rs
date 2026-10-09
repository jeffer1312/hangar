#![cfg(unix)]
use hangar_server::terminal_input::*;
use std::{sync::{Arc,atomic::{AtomicU32,Ordering}},time::{Duration,SystemTime,UNIX_EPOCH}};
use tokio::process::Command;
struct Facts(TerminalBinding);
impl TerminalServices for Facts {
 fn facts<'a>(&'a self,_:&'a TerminalBinding)->ServiceFuture<'a,InputFacts>{Box::pin(async{Ok(InputFacts{binding:self.0.clone(),ready:true,idle:true,open_question:false,plugin_live:false,plugin_user:false,clipboard_available:true,native:None})})}
 fn publish<'a>(&'a self,_:&'a TerminalBinding,_:PluginRequest)->ServiceFuture<'a,PluginReply>{Box::pin(async{Ok(PluginReply::Unavailable)})}
}
struct IsolatedMux(String);
impl Drop for IsolatedMux {fn drop(&mut self){let _=std::process::Command::new("tmux").args(["-L",&self.0,"kill-server"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();}}
const FAKE_CLI:&str=r#"import os, sys, tty, termios, json, codecs
fd=sys.stdin.fileno(); old=termios.tcgetattr(fd); tty.setraw(fd)
text=''; incoming=''; messages=[]; stash=None
def render():
 hint='  › stashed\r\n' if stash is not None else ''
 sys.stdout.write('\x1b[2J\x1b[Hhistory\r\n'+hint+'─'*40+'\r\n❯ '+text.replace('\n','\r\n')+'\r\n'+'─'*40+'\r\n⏵⏵ bypass permissions\r\n'); sys.stdout.flush()
sys.stdout.write('\x1b[?2004h'); render(); decoder=codecs.getincrementaldecoder('utf-8')()
try:
 while True:
  incoming+=decoder.decode(os.read(fd,4096))
  while incoming:
   if incoming.startswith('\x1b[200~'):
    end=incoming.find('\x1b[201~')
    if end<0: break
    text+=incoming[6:end].replace('\r','\n'); incoming=incoming[end+6:]; render(); continue
   if incoming.startswith('\x1b') and len(incoming)<6: break
   c=incoming[0]; incoming=incoming[1:]
   if c=='\r':
    messages.append(text); open(sys.argv[1],'w',encoding='utf-8').write(json.dumps(messages,ensure_ascii=False)); text=''
    # Como o Claude: o envio devolve o guardado ao composer.
    if stash is not None: text=stash; stash=None
   elif c=='\x13':
    # Ctrl+S: uma vaga só; com texto guarda (por cima do que houver), vazio devolve.
    if text: stash=text; text=''
    elif stash is not None: text=stash; stash=None
   elif c=='\x15': text=''
   else: text+=c
   render()
finally: termios.tcsetattr(fd,termios.TCSADRAIN,old)
"#;
struct FakeCli {_dir:tempfile::TempDir,receipt:std::path::PathBuf,err:std::path::PathBuf,label:String,_guard:IsolatedMux,driver:TerminalDriver}
async fn fake_cli() -> FakeCli {
 let dir=tempfile::tempdir().unwrap();let cli=dir.path().join("fake_cli.py");let receipt=dir.path().join("receipt.json");let err=dir.path().join("fake_cli.err");
 std::fs::write(&cli,FAKE_CLI).unwrap();
 // Os testes rodam em paralelo no mesmo processo; o relógio do macOS tem passo de 1 µs e dois
 // rótulos por horário colidiam, e o Drop de um derrubava o tmux do outro.
 static NEXT:AtomicU32=AtomicU32::new(0);
 let label=format!("hangar-input-test-{}-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),NEXT.fetch_add(1,Ordering::Relaxed));let guard=IsolatedMux(label.clone());
 // O stderr do CLI falso vai a um arquivo: quando o pane morre cedo, é a única pista do motivo.
 let cli_cmd=format!("python3 '{}' '{}' 2>'{}'",cli.display(),receipt.display(),err.display());
 // -f /dev/null: o ~/.tmux.conf de quem roda (base-index 1, por exemplo) muda o alvo =test:0.0.
 let new=Command::new("tmux").args(["-L",&label,"-f","/dev/null","new-session","-d","-s","test","-x","100","-y","40",&cli_cmd]).output().await.unwrap();assert!(new.status.success(),"new-session: {}; fake_cli stderr: {}",String::from_utf8_lossy(&new.stderr),std::fs::read_to_string(&err).unwrap_or_default());
 let meta=Command::new("tmux").args(["-L",&label,"display-message","-p","-t","=test:0.0","#{pane_id}\t#{session_created}"]).output().await.unwrap();assert!(meta.status.success());let meta=String::from_utf8(meta.stdout).unwrap();let mut fields=meta.trim().split('\t');let pane=fields.next().unwrap().to_string();let created=fields.next().unwrap().parse().unwrap();
 let binding=TerminalBinding{name:"test".into(),pane,conversation:"fake-conversation".into(),generation:1,created,mux_argv:vec!["tmux".into(),"-L".into(),label.clone()],windows:false,clipboard_lock_path:None};
 let driver=TerminalDriver::new(binding.clone(),Arc::new(Facts(binding)),Arc::new(ProcessIo::default()),InputLimits{settle:Duration::from_millis(10),literal_settle:Duration::from_millis(25),multiline_settle:Duration::from_millis(25),slash_settle:Duration::from_millis(25),proof_attempts:40,ready_attempts:40,cleanup_attempts:4});
 for _ in 0..100 {if driver.capture().await.is_ok_and(|s|ComposerSnapshot::parse(&s).is_some()){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
 FakeCli {_dir:dir,receipt,err,label,_guard:guard,driver}
}
#[tokio::test]
async fn terminal_input_tmux_isolated_fake_cli_unicode_multiline_and_clear() {
 let f=fake_cli().await;let d=&f.driver;
 for (id,text) in [("short","ok"),("unicode","ação 😀 C:\\Users\\test"),("multiline","first\nsecond\nthird"),("clear","/clear"),("semicolon","literal;")] {let r=d.prompt(text,id).await;assert_eq!(r.disposition,Disposition::Accepted,"{id}: {}; fake capture={:?}; fake_cli stderr: {}",r.code,d.capture().await,std::fs::read_to_string(&f.err).unwrap_or_default());}
 let received:Vec<String>=serde_json::from_slice(&std::fs::read(&f.receipt).unwrap()).unwrap();assert_eq!(received,vec!["ok","ação 😀 C:\\Users\\test","first\nsecond\nthird","/clear","literal;"]);
}
#[tokio::test]
async fn terminal_input_tmux_owner_draft_is_stashed_and_comes_back_identical() {
 let f=fake_cli().await;let d=&f.driver;let tmux=|args:&[&str]|{let mut a=vec!["-L",f.label.as_str()];a.extend_from_slice(args);std::process::Command::new("tmux").args(a).output().unwrap()};
 // O dono deixou um rascunho de várias linhas, colado, sem Enter.
 let draft="rascunho do dono\nação \"aspas\" $HOME \\ C:\\x;";
 let mut load=std::process::Command::new("tmux").args(["-L",&f.label,"load-buffer","-b","dono","-"]).stdin(std::process::Stdio::piped()).spawn().unwrap();
 std::io::Write::write_all(load.stdin.as_mut().unwrap(),draft.as_bytes()).unwrap();assert!(load.wait().unwrap().success());
 assert!(tmux(&["paste-buffer","-t","=test:0.0","-b","dono","-p","-d"]).status.success());
 for _ in 0..100 {if d.capture().await.is_ok_and(|s|s.contains("rascunho do dono")){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
 let r=d.prompt("mensagem do app","app").await;
 assert_eq!(r.disposition,Disposition::Accepted,"{}; capture={:?}; fake_cli stderr: {}",r.code,d.capture().await,std::fs::read_to_string(&f.err).unwrap_or_default());
 assert_eq!(r.draft,Some(DraftOutcome::Returned));
 // O Enter do dono manda o rascunho de volta igual: nada dele foi para a mensagem do app.
 assert!(tmux(&["send-keys","-t","=test:0.0","-l","--","\r"]).status.success());
 let mut received:Vec<String>=vec![];
 for _ in 0..200 {received=std::fs::read(&f.receipt).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();if received.len()==2{break;}tokio::time::sleep(Duration::from_millis(10)).await;}
 assert_eq!(received,vec!["mensagem do app".to_string(),draft.to_string()]);
}
const MOUSE_CLI:&str=r#"import os, sys, tty
fd=sys.stdin.fileno(); tty.setraw(fd)
sys.stdout.write('\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[Hmouse\r\n'); sys.stdout.flush()
out=open(sys.argv[1],'ab',buffering=0)
while True:
 data=os.read(fd,4096)
 if not data: break
 out.write(data)
"#;
#[tokio::test]
async fn terminal_input_tmux_mouse_and_size_reach_the_program() {
 let dir=tempfile::tempdir().unwrap();let cli=dir.path().join("mouse_cli.py");let received=dir.path().join("received.bin");
 std::fs::write(&cli,MOUSE_CLI).unwrap();
 let label=format!("hangar-mods-test-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());let _guard=IsolatedMux(label.clone());
 let new=Command::new("tmux").args(["-L",&label,"-f","/dev/null","new-session","-d","-s","test","-x","100","-y","40",&format!("python3 '{}' '{}'",cli.display(),received.display())]).output().await.unwrap();assert!(new.status.success());
 let meta=Command::new("tmux").args(["-L",&label,"display-message","-p","-t","=test:","#{pane_id}\t#{session_created}"]).output().await.unwrap();let meta=String::from_utf8(meta.stdout).unwrap();let mut fields=meta.trim().split('\t');
 let binding=TerminalBinding{name:"test".into(),pane:fields.next().unwrap().into(),conversation:"c".into(),generation:1,created:fields.next().unwrap().parse().unwrap(),mux_argv:vec!["tmux".into(),"-L".into(),label.clone()],windows:false,clipboard_lock_path:None};
 let d=TerminalDriver::new(binding.clone(),Arc::new(Facts(binding)),Arc::new(ProcessIo::default()),InputLimits::default());
 for _ in 0..100 {if d.mods_formats().await.is_ok_and(|f|f.mouse){break;}tokio::time::sleep(Duration::from_millis(20)).await;}
 assert_eq!(d.mods_formats().await.unwrap(),PaneFormats{mouse:true,in_mode:false,columns:100,rows:40});
 d.mouse(2,9).await.unwrap();
 let want=b"\x1b[<0;10;3M\x1b[<0;10;3m";
 for _ in 0..100 {if std::fs::read(&received).is_ok_and(|b|b.windows(want.len()).any(|w|w==want)){break;}tokio::time::sleep(Duration::from_millis(20)).await;}
 assert!(std::fs::read(&received).unwrap().windows(want.len()).any(|w|w==want),"o programa recebeu o clique SGR na célula pedida");
 d.resize(144,45).await.unwrap();
 let size=Command::new("tmux").args(["-L",&label,"display-message","-p","-t","=test:","#{window_width}x#{window_height}"]).output().await.unwrap();
 assert_eq!(String::from_utf8(size.stdout).unwrap().trim(),"144x45");
 let option=Command::new("tmux").args(["-L",&label,"show-options","-w","-t","=test:","window-size"]).output().await.unwrap();
 assert_eq!(String::from_utf8(option.stdout).unwrap().trim(),"window-size latest","o tamanho volta a ser de quem se ligar");
 // Um cliente de controle (como o observador da prévia) entra no session_attached, mas não é terminal ligado.
 let mut control=std::process::Command::new("tmux").args(["-L",&label,"-C","attach-session","-f","ignore-size,no-output","-t","=test"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
 for _ in 0..100 {let a=Command::new("tmux").args(["-L",&label,"display-message","-p","-t","=test:","#{session_attached}"]).output().await.unwrap();if String::from_utf8(a.stdout).unwrap().trim()=="1"{break;}tokio::time::sleep(Duration::from_millis(20)).await;}
 assert_eq!(d.mods_clients().await.unwrap(),0);
 let _=control.kill();let _=control.wait();
}
